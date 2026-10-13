use super::{PinnedExecutor, PluginId, PluginInstruction};
use crate::pipes::{
    apply_pipe_message_to_plugin, pipes_to_block_or_unblock, report_pipe_changes, PendingPipes,
    PipeOutcomes,
};
use crate::plugin_loader::PluginLoader;
use crate::plugin_map::{
    AtomicEvent, PluginEnv, PluginMap, RunningPlugin, RunningPluginEntry, Subscriptions,
};
use crate::{PipeStateChange, PluginRenderAsset};
use yinz_bus::plugins::{PipeCompletion, PipeRequestId, PluginInstanceId};
use yinz_types::internal_ids::QueueJobId;
use yinz_types::internal_ids::TabId;

use crate::plugin_worker::MessageToWorker;
use crate::queues::{CancelResult, FinishedJob, QueuedJob, Queues};
use log::info;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard, PoisonError, TryLockError, Weak,
    },
    time::{Duration, Instant},
};
use tokio::sync::mpsc::Sender;
use url::Url;
use wasmi::{Engine, Module};
use yinz_config::permission::PermissionCache;
use yinz_runtime::owned_dir::OwnedDir;
use yinz_storage::data_access::DataAccess;
use yinz_types::geometry::FloatingPaneCoordinates;
use yinz_types::input::keybinds::Keybinds;
use yinz_types::style::HostTerminalThemeMode;
use {yinz_runtime::consts::YINZ_CACHE_DIR, yinz_runtime::consts::YINZ_TMP_DIR};
use {
    yinz_types::data::InputMode, yinz_types::data::KeybindsVec, yinz_types::data::LayoutInfo,
    yinz_types::data::LayoutWithError, yinz_types::data::PaneContents,
    yinz_types::data::PaneRenderReport, yinz_types::data::PermissionStatus,
    yinz_types::data::PermissionType, yinz_types::data::PipeMessage, yinz_types::data::PipeSource,
    yinz_types::data::QueueJobOutcome,
};

use crate::ClientId;
use crate::PaneId;
use yinz_bus::{
    background_jobs::BackgroundJob, loading_indication::LoadingIndication, route::NotificationEnd,
    screen::ScreenInstruction, thread_bus::ThreadSenders, ServerInstruction,
};
use yinz_runtime::errors::prelude::*;
use yinz_types::pane_size::Size;
use {
    yinz_config::plugins::PluginConfig, yinz_types::data::Event, yinz_types::data::EventType,
    yinz_types::input::command::TerminalAction, yinz_types::input::layout::PluginUserConfiguration,
    yinz_types::input::layout::RunPlugin, yinz_types::input::layout::RunPluginLocation,
    yinz_types::input::layout::RunPluginOrAlias, yinz_types::input::plugins::PluginAliases,
};

fn make_plugin_url_path_safe(url: String) -> String {
    url
}

#[derive(Debug, Clone)]
pub enum EventOrPipeMessage {
    Event(Event),
    DenoEvent(
        ClientId,
        u64,
        Event,
        Option<yinz_bus::thread_bus::DenoReplyId>,
    ),
    // The cached delivery owns the pipe until replay registers the actual instances.
    PipeMessage(Option<ClientId>, PipeMessage, Option<PipeRequestId>, u64),
}

struct PipeDelivery {
    plugin: PluginId,
    instance_id: PluginInstanceId,
    instance: Arc<Mutex<RunningPlugin>>,
    client: ClientId,
    render_clients: Vec<ClientId>,
    message: PipeMessage,
    request: Option<PipeRequestId>,
}

impl EventOrPipeMessage {
    fn client_for_replay(
        &self,
        owner: ClientId,
        generation: Option<u64>,
        shared: bool,
    ) -> Option<ClientId> {
        match self {
            Self::DenoEvent(client, expected, _, _)
                if generation == Some(*expected) && (shared || *client == owner) =>
            {
                Some(*client)
            },
            Self::DenoEvent(..) => None,
            _ => Some(owner),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LoadingContext {
    pub plugin_roles: crate::plugin_map::PluginRoles,
    pub pick_request: Option<yinz_types::pick::PickRequest>,
    pub pick_cancelled: Option<Arc<AtomicBool>>,
    pub pick_opening: Option<Arc<AtomicBool>>,
    pub plugin_id: PluginId,
    pub client_id: ClientId,
    pub plugin_cwd: PathBuf,
    pub plugin_own_data_dir: PathBuf,
    pub temporary_data_dir: Arc<OwnedDir>,
    pub plugin_own_cache_dir: PathBuf,
    pub plugin_config: PluginConfig,
    pub data_owner: Url,
    pub tab_index: Option<TabId>,
    pub path_to_default_shell: PathBuf,
    pub session_env_vars: std::collections::BTreeMap<String, String>,
    pub server_session: crate::ServerSession,
    pub plugin_aliases: Arc<Mutex<PluginAliases>>,
    pub default_shell: Option<TerminalAction>,
    pub layout_dir: Option<PathBuf>,
    pub user_plugin_dir: Option<PathBuf>,
    pub builtin_plugins_dir: Option<PathBuf>,
    pub default_mode: InputMode,
    pub keybinds: Keybinds,
    pub yinz_data_dir: PathBuf,
    pub data_access: Arc<DataAccess>,
    pub deno_watch_generations: super::plugin_map::DenoWatchGenerations,
    pub queues: Arc<Mutex<Queues>>,
    pub size: Size,
    pub cancel: Arc<AtomicBool>,
    pub client_cancel: Arc<AtomicBool>,
    pub client_cancellations: Arc<Mutex<HashMap<ClientId, Arc<AtomicBool>>>>,
}

impl LoadingContext {
    /// Git runs outside both the executor and plugin map. The queued context pins the revision.
    fn execute_prepared<F>(
        mut self,
        executor: Arc<PinnedExecutor>,
        senders: ThreadSenders,
        load_id: u64,
        register: bool,
        job: F,
    ) -> bool
    where
        F: FnOnce(
                Self,
                ThreadSenders,
                Arc<Mutex<PluginMap>>,
                Arc<Mutex<Vec<ClientId>>>,
                PluginCache,
                Engine,
            ) + Send
            + 'static,
    {
        let plugin_id = self.plugin_id;
        if register {
            executor.register_plugin(plugin_id);
        }
        let needs_repository = matches!(self.plugin_config.location, RunPluginLocation::Git(_))
            && self.plugin_config.repository_root.is_none();
        let enqueue = move || {
            if let RunPluginLocation::Git(source) = &self.plugin_config.location {
                if self.plugin_config.repository_root.is_none() {
                    let entries = self
                        .plugin_aliases
                        .lock()
                        .unwrap()
                        .aliases
                        .values()
                        .filter_map(|plugin| match &plugin.location {
                            RunPluginLocation::Git(other)
                                if other.repo == source.repo
                                    && other.revision == source.revision =>
                            {
                                Some(other.clone())
                            },
                            _ => None,
                        })
                        .collect::<Vec<_>>();
                    match yinz_plugin_authoring::git::resolve(
                        &self.yinz_data_dir,
                        source,
                        &entries,
                        &self.cancel,
                    ) {
                        Ok(installed) => {
                            self.plugin_config.path = installed.entrypoint;
                            self.plugin_config.repository_root = Some(installed.root);
                            self.plugin_config.installed_commit = installed.commit;
                        },
                        Err(error) => {
                            handle_plugin_loading_failure(
                                &senders,
                                Some(load_id),
                                plugin_id,
                                &mut LoadingIndication::new(source.descriptor()),
                                error.context(
                                    crate::plugin_authoring::LogIdentity::repository(source)
                                        .marker(),
                                ),
                                Some(self.client_id),
                            );
                            return true;
                        },
                    }
                }
            }
            let work = move |senders, map, clients, cache, engine| {
                job(self, senders, map, clients, cache, engine)
            };
            let accepted = if register {
                executor.execute_plugin_load(plugin_id, work)
            } else {
                executor.execute_for_plugin_tagged(plugin_id, "load", false, work)
            };
            if !accepted {
                handle_plugin_loading_failure(
                    &senders,
                    Some(load_id),
                    plugin_id,
                    &mut LoadingIndication::new(plugin_id.to_string()),
                    "Plugin executor rejected loading",
                    None,
                );
            }
            accepted
        };
        if needs_repository {
            std::thread::Builder::new()
                .name(format!("plugin-repository-{plugin_id}"))
                .spawn(enqueue)
                .is_ok()
        } else {
            enqueue()
        }
    }

    pub fn new(
        wasm_bridge: &WasmBridge,
        cwd: Option<PathBuf>,
        plugin_config: PluginConfig,
        plugin_id: PluginId,
        client_id: ClientId,
        tab_index: Option<TabId>,
        size: Size,
    ) -> Result<Self> {
        let temporary_data_dir = wasm_bridge
            .temporary_root
            .child()
            .context("failed to create plugin /data")?;
        let plugin_own_data_dir = temporary_data_dir.path().to_path_buf();
        // Canonical file identity can change on reload; /cache belongs to the existing plugin.
        let plugin_own_cache_dir = wasm_bridge
            .plugin_map
            .lock()
            .unwrap()
            .cache_dir_of_plugin_id(plugin_id)
            .unwrap_or_else(|| {
                YINZ_CACHE_DIR
                    .join(make_plugin_url_path_safe(
                        Url::from(&plugin_config.location).to_string(),
                    ))
                    .join("plugin_cache")
            });
        let default_mode = wasm_bridge
            .base_modes
            .get(&client_id)
            .copied()
            .unwrap_or(wasm_bridge.default_mode);
        let keybinds = wasm_bridge
            .keybinds
            .get(&client_id)
            .cloned()
            .unwrap_or_else(|| wasm_bridge.default_keybinds.clone());
        let plugin_cwd = cwd.unwrap_or_else(|| wasm_bridge.yinz_cwd.clone());
        let data_owner;
        {
            let mut plugin_map = wasm_bridge.plugin_map.lock().unwrap();
            let previous = plugin_map.reload_metadata(plugin_id);
            data_owner = previous
                .as_ref()
                .map(|metadata| metadata.data_owner.clone())
                .unwrap_or_else(|| Url::from(&plugin_config.location));
            let resolved_source_path = previous.and_then(|metadata| metadata.resolved_source_path);
            plugin_map.remember_plugin(
                plugin_id,
                super::plugin_map::PluginMetadata {
                    config: plugin_config.clone(),
                    data_owner: data_owner.clone(),
                    resolved_source_path,
                    cache_dir: plugin_own_cache_dir.clone(),
                    cwd: plugin_cwd.clone(),
                    tab_index,
                    size: (size.rows, size.cols),
                    owner_client_id: client_id,
                },
            );
        }

        Ok(LoadingContext {
            plugin_roles: wasm_bridge.plugin_roles.clone(),
            pick_opening: wasm_bridge
                .picks
                .get(&plugin_id)
                .map(|pick| pick.opening.clone()),
            pick_cancelled: wasm_bridge
                .picks
                .get(&plugin_id)
                .map(|pick| pick.cancelled.clone()),
            pick_request: wasm_bridge
                .picks
                .get(&plugin_id)
                .map(|pick| pick.request.clone()),
            client_id,
            plugin_id,
            path_to_default_shell: wasm_bridge.path_to_default_shell.clone(),
            session_env_vars: wasm_bridge.session_env_vars.clone(),
            server_session: wasm_bridge.server_session.clone(),
            plugin_aliases: wasm_bridge.plugin_aliases.clone(),
            plugin_cwd,
            default_shell: wasm_bridge.default_shell.clone(),
            layout_dir: wasm_bridge.layout_dir.clone(),
            user_plugin_dir: wasm_bridge.user_plugin_dir.clone(),
            builtin_plugins_dir: wasm_bridge.builtin_plugins_dir.clone(),
            keybinds,
            default_mode,
            plugin_own_data_dir,
            temporary_data_dir,
            plugin_own_cache_dir,
            plugin_config,
            data_owner,
            tab_index,
            yinz_data_dir: wasm_bridge.yinz_data_dir.clone(),
            data_access: wasm_bridge.data_access.clone(),
            deno_watch_generations: wasm_bridge.deno_watch_generations.clone(),
            queues: wasm_bridge.queues.clone(),
            size,
            // Made by the load or reload this context is for; a context built for a plugin
            // nothing registered (an `add_client` for one mid-unload) gets a token nobody sets.
            cancel: wasm_bridge
                .cancel_tokens
                .get(&plugin_id)
                .cloned()
                .unwrap_or_default(),
            client_cancel: {
                let connected = wasm_bridge.client_is_connected(&client_id);
                let mut cancellations = wasm_bridge.client_cancellations.lock().unwrap();
                let cancel = cancellations.entry(client_id).or_default();
                if !connected && cancel.load(Ordering::Acquire) {
                    // A fresh CLI/headless load may reuse a detached UI's id. Its work
                    // has its own lifetime; the old UI token must remain cancelled.
                    Arc::default()
                } else {
                    // Startup loads precede AddClient and must share its attachment token.
                    cancel.clone()
                }
            },
            client_cancellations: wasm_bridge.client_cancellations.clone(),
        })
    }
}

pub type PluginCacheKey = (PathBuf, Option<PathBuf>);
pub type PluginCache = Arc<Mutex<HashMap<PluginCacheKey, (Module, PathBuf)>>>;

#[path = "background_plugins.rs"]
mod background_plugins;
#[path = "permission_requests.rs"]
mod permission_requests;
#[path = "picker.rs"]
pub(crate) mod picker;

pub struct WasmBridge {
    picks: HashMap<PluginId, picker::PendingPick>,
    permission_requests: BTreeMap<u64, permission_requests::PendingPermission>,
    next_permission_request: u64,
    pub(crate) plugin_roles: crate::plugin_map::PluginRoles,
    independent_plugin_ids: HashSet<PluginId>,
    configured_plugins: Vec<RunPlugin>,
    resolved_plugin_locations: HashMap<RunPluginLocation, RunPluginLocation>,
    latest_plugin_errors: HashMap<PluginId, String>,
    plugin_callers: HashMap<PluginId, yinz_types::internal_ids::TerminalId>,
    temporary_root: Arc<OwnedDir>,
    temporary_cleanup_done: Option<std::sync::mpsc::Receiver<()>>,
    connected_clients: Arc<Mutex<Vec<ClientId>>>,
    senders: ThreadSenders,
    yinz_data_dir: PathBuf,
    data_access: Arc<DataAccess>,
    deno_watch_generations: super::plugin_map::DenoWatchGenerations,
    pub(super) plugin_map: Arc<Mutex<PluginMap>>,
    plugin_executor: Arc<PinnedExecutor>,
    next_plugin_id: u32,
    next_load_id: u64,
    // Last scheduled attempt: a delayed load result must not settle a replacement cache.
    load_ids: HashMap<PluginId, u64>,
    // plugins that run as a single instance for the whole session rather than one per client
    shared_plugin_ids: HashSet<PluginId>,
    plugin_ids_waiting_for_permission_request: HashSet<PluginId>,
    /// `yinz events` clients, each with the event types it asked for; an empty set is every
    /// streamable one. Kept here rather than on the screen because this thread is the one place
    /// every event passes through, whichever thread produced it.
    cli_event_subscribers: HashMap<ClientId, HashSet<EventType>>,
    cached_events_for_pending_plugins: HashMap<PluginId, Vec<EventOrPipeMessage>>,
    cached_resizes_for_pending_plugins: HashMap<PluginId, (usize, usize)>, // (rows, columns)
    cached_worker_messages: HashMap<PluginId, Vec<(ClientId, String, String, String)>>, // Vec<clientid,
    // worker_name,
    // message,
    // payload>
    loading_plugins: HashSet<(PluginId, RunPlugin)>, // tracks loading plugins without handles
    /// Plugins unloaded before their loader ran. `unload_plugin` marks a plugin here when it is
    /// in `loading_plugins` but not yet in the `PluginMap`; the loader closure checks the mark
    /// and gives up instead of instantiating. Both sides do so **under the `plugin_map` lock**,
    /// which the loader holds from before instantiation until after `load` returns, so an unload
    /// either finds the runtime in the map or the loader finds the mark - never neither. Do not
    /// move either check outside that lock.
    unloaded_while_loading: Arc<Mutex<HashSet<PluginId>>>,
    pending_plugin_reloads: HashSet<RunPlugin>,
    pending_plugin_id_reloads: HashSet<PluginId>,
    /// One cancel token per plugin, shared with every instance's `PluginEnv::cancel`. Kept here
    /// so that `unload_plugin` can set it **without taking the plugin's lock** — an instance
    /// looping inside `update()` holds that lock on its executor thread, and the plugin thread
    /// waiting for it there is exactly how a runaway plugin once wedged session shutdown.
    cancel_tokens: HashMap<PluginId, Arc<AtomicBool>>,
    // Attachment-scoped: a reused client ID must not revive pending work from its
    // previous connection. Separate from plugin cancellation so peers keep running.
    client_cancellations: Arc<Mutex<HashMap<ClientId, Arc<AtomicBool>>>>,
    /// Named work queues. Jobs are ordinary plugins once running; this is what decides when
    /// they start and who is told when they end. Shared with every `PluginEnv`, which enqueues
    /// and reads status directly - see the note on `PluginEnv::queues`. Never hold this lock
    /// across `load_plugin` or `unload_plugin`.
    queues: Arc<Mutex<Queues>>,
    path_to_default_shell: PathBuf,
    yinz_cwd: PathBuf,
    session_env_vars: std::collections::BTreeMap<String, String>,
    server_session: crate::ServerSession,
    default_shell: Option<TerminalAction>,
    pending_pipes: PendingPipes,
    layout_dir: Option<PathBuf>,
    user_plugin_dir: Option<PathBuf>,
    builtin_plugins_dir: Option<PathBuf>,
    available_layouts: Vec<LayoutInfo>,
    available_layout_errors: Vec<LayoutWithError>,
    default_mode: InputMode,
    default_keybinds: Keybinds,
    keybinds: HashMap<ClientId, Keybinds>,
    base_modes: HashMap<ClientId, InputMode>,
    previous_pane_render_report: Option<PaneRenderReport>,
    /// The last `HostTerminalThemeChanged` broadcast to every plugin, replayed
    /// to a plugin that subscribes to it after the fact so it does not wait
    /// for the next host report to learn the session's hue.
    last_host_terminal_theme_mode: Option<HostTerminalThemeMode>,
    pub last_session_save_time: Arc<Mutex<Option<u64>>>, // milliseconds since UNIX epoch
    /// The `plugins { }` block, for plugins asking what a *different* alias was
    /// configured with (`PluginCommand::GetPluginAliasConfiguration`).
    ///
    /// Shared rather than cloned into each `PluginEnv` because it is the one piece of
    /// plugin state that must survive a config reload: `detect_and_notify_plugin_config_changes`
    /// is handed the new table and writes it here, so a plugin that asks after the reload
    /// is answered with what the user just wrote rather than with what was on disk at
    /// server start.
    pub plugin_aliases: Arc<Mutex<PluginAliases>>,
}

fn close_plugin_instance(
    plugin_id: PluginId,
    client_id: ClientId,
    running_plugin: Arc<Mutex<RunningPlugin>>,
    subscriptions: Arc<Mutex<Subscriptions>>,
    senders: ThreadSenders,
) {
    let subscriptions_guard = subscriptions.lock().unwrap();
    let needs_before_close = subscriptions_guard.contains(&EventType::BeforeClose);
    drop(subscriptions_guard); // Release lock before calling plugin

    if needs_before_close && !running_plugin.lock().unwrap().is_disposed() {
        let mut rp = running_plugin.lock().unwrap();
        // Holding the lock means the guest is not running. If the token was set but
        // never tripped, the guest was idle and `BeforeClose` gets a normal call. If
        // it tripped, the guest was stopped mid-call, and is not called again: see
        // `PluginEnv::cancel_tripped`.
        let was_stopped_mid_call = rp.env().cancel_tripped.load(Ordering::Relaxed);
        rp.env().cancel.store(false, Ordering::Relaxed);
        if was_stopped_mid_call || rp.failed {
            log::warn!(
                "plugin {plugin_id} was cancelled mid-call, so its BeforeClose is \
                             skipped"
            );
        } else {
            match apply_before_close_event_to_plugin(plugin_id, client_id, &mut rp, senders.clone())
            {
                Ok(()) => {},
                Err(e) => {
                    log::error!("{:?}", e);
                    let stringified_error = format!("{:?}", e).replace("\n", "\n\r");
                    if !rp.is_disposed() {
                        handle_plugin_crash(
                            plugin_id,
                            rp.env().deno_generation,
                            Some(rp.instance_id),
                            stringified_error,
                            senders.clone(),
                        );
                    }
                },
            }
        }
    }

    running_plugin.lock().unwrap().dispose();
    let _ = senders.send_to_screen(ScreenInstruction::SubscribePaneOutput(
        plugin_id, client_id, None, None,
    ));
    drop(running_plugin);
    drop(subscriptions);
}

fn cleanup_disconnected_instances(
    plugin_id: PluginId,
    connected_clients: &Arc<Mutex<Vec<ClientId>>>,
    plugin_map: &Arc<Mutex<PluginMap>>,
    senders: ThreadSenders,
) {
    let removed = {
        // Match the loader's lock order. Registration happens before instance lookup,
        // so a reconnect either keeps its instance or sees it removed and loads one.
        let mut plugin_map = plugin_map.lock().unwrap();
        let connected = connected_clients.lock().unwrap();
        plugin_map.remove_disconnected_instances(plugin_id, &connected)
    };
    for (client_id, (plugin, subscriptions, workers)) in removed {
        for sender in workers.into_values() {
            let _ = sender.send(MessageToWorker::Exit);
        }
        close_plugin_instance(plugin_id, client_id, plugin, subscriptions, senders.clone());
    }
}

impl Drop for WasmBridge {
    fn drop(&mut self) {
        self.cancel_all_plugins();
    }
}

impl WasmBridge {
    fn begin_plugin_load(&mut self, plugin_id: PluginId) -> u64 {
        self.latest_plugin_errors.remove(&plugin_id);
        self.next_load_id += 1;
        self.load_ids.insert(plugin_id, self.next_load_id);
        self.next_load_id
    }

    pub(crate) fn load_is_current(&self, plugin_id: PluginId, load_id: u64) -> bool {
        self.load_ids.get(&plugin_id) == Some(&load_id)
    }

    pub fn new(
        senders: ThreadSenders,
        engine: Engine,
        yinz_data_dir: PathBuf,
        data_access: Arc<DataAccess>,
        path_to_default_shell: PathBuf,
        yinz_cwd: PathBuf,
        session_env_vars: std::collections::BTreeMap<String, String>,
        default_shell: Option<TerminalAction>,
        layout_dir: Option<PathBuf>,
        user_plugin_dir: Option<PathBuf>,
        builtin_plugins_dir: Option<PathBuf>,
        available_layouts: Vec<LayoutInfo>,
        available_layout_errors: Vec<LayoutWithError>,
        default_mode: InputMode,
        default_keybinds: Keybinds,
        plugin_aliases: PluginAliases,
        server_session: crate::ServerSession,
    ) -> Result<Self> {
        let mut temporary_root = OwnedDir::new(&YINZ_CACHE_DIR.join("plugin-tmp"))
            .context("failed to create plugin host temporary directory")?;
        let temporary_cleanup_done = Some(
            temporary_root
                .drop_notification()
                .context("failed to observe plugin host cleanup")?,
        );
        let plugin_map = Arc::new(Mutex::new(PluginMap::default()));
        let connected_clients: Arc<Mutex<Vec<ClientId>>> = Arc::new(Mutex::new(vec![]));
        let plugin_cache: PluginCache = Arc::new(Mutex::new(HashMap::new()));
        let max_threads = num_cpus::get().clamp(4, 16);
        let plugin_executor = Arc::new(PinnedExecutor::new(
            max_threads,
            &senders,
            &plugin_map,
            &connected_clients,
            &plugin_cache,
            &engine,
        ));
        Ok(WasmBridge {
            permission_requests: BTreeMap::new(),
            next_permission_request: 0,
            plugin_roles: Default::default(),
            picks: HashMap::new(),
            independent_plugin_ids: HashSet::new(),
            configured_plugins: vec![],
            resolved_plugin_locations: HashMap::new(),
            latest_plugin_errors: HashMap::new(),
            plugin_callers: HashMap::new(),
            temporary_root,
            temporary_cleanup_done,
            connected_clients,
            senders,
            yinz_data_dir,
            data_access,
            deno_watch_generations: Default::default(),
            plugin_map,
            plugin_executor,
            path_to_default_shell,
            next_plugin_id: 0,
            next_load_id: 0,
            load_ids: HashMap::new(),
            shared_plugin_ids: HashSet::new(),
            cached_events_for_pending_plugins: HashMap::new(),
            plugin_ids_waiting_for_permission_request: HashSet::new(),
            cached_resizes_for_pending_plugins: HashMap::new(),
            cached_worker_messages: HashMap::new(),
            cli_event_subscribers: HashMap::new(),
            loading_plugins: HashSet::new(),
            unloaded_while_loading: Default::default(),
            pending_plugin_reloads: HashSet::new(),
            pending_plugin_id_reloads: HashSet::new(),
            cancel_tokens: HashMap::new(),
            client_cancellations: Arc::default(),
            queues: Default::default(),
            yinz_cwd,
            session_env_vars,
            server_session,
            default_shell,
            pending_pipes: Default::default(),
            layout_dir,
            user_plugin_dir,
            builtin_plugins_dir,
            available_layouts,
            available_layout_errors,
            default_mode,
            default_keybinds,
            keybinds: HashMap::new(),
            base_modes: HashMap::new(),
            previous_pane_render_report: None,
            last_host_terminal_theme_mode: None,
            last_session_save_time: Arc::new(Mutex::new(None)),
            plugin_aliases: Arc::new(Mutex::new(plugin_aliases)),
        })
    }
    pub fn load_plugin(
        &mut self,
        run: &Option<RunPlugin>,
        tab_index: Option<TabId>,
        size: Size,
        cwd: Option<PathBuf>,
        skip_cache: bool,
        client_id: Option<ClientId>,
    ) -> Result<(PluginId, ClientId)> {
        let _err_context = move || "failed to load plugin".to_string();
        let run = run.clone();
        if let Some(run) = &run {
            self.remember_plugin_location(run);
        }

        let client_id = client_id
            .and_then(|client_id| {
                // first attempt to use a connected client (because this might be a cli_client that
                // should not get plugins) and only if none is connected, load a "dummy" plugin for
                // the cli client
                let connected_clients = self.connected_clients.lock().unwrap();
                if connected_clients.contains(&client_id) {
                    Some(client_id)
                } else {
                    None
                }
            })
            .or_else(|| {
                // if no client id was provided, try to use the first connected client
                self.connected_clients
                    .lock()
                    .unwrap()
                    .iter()
                    .next()
                    .copied()
            })
            .or(client_id) // if we got here, this is likely a cli client with no other clients
            // connected, or loading a background plugin on app start, we use the provided client id as a dummy to load the
            // plugin anyway
            .with_context(|| {
                "Plugins must have a client id, none was provided and none are connected"
            })?;

        let plugin_id = PluginId::new(self.next_plugin_id);
        self.plugin_roles
            .lock()
            .unwrap()
            .entry(plugin_id)
            .or_insert(false);
        self.independent_plugin_ids.insert(plugin_id);
        self.cached_events_for_pending_plugins
            .insert(plugin_id, vec![]);

        match run {
            Some(run) => {
                // Even resolution failures complete asynchronously. Keep their initial
                // pipes pending until the failure instruction supplies the diagnostic.
                self.loading_plugins.insert((plugin_id, run.clone()));
                let plugin_config = match PluginConfig::from_run_plugin(&run) {
                    Some(plugin_config) => plugin_config,
                    None => {
                        self.next_plugin_id += 1;
                        let mut loading_indication =
                            LoadingIndication::new(run.location.to_string());
                        handle_plugin_loading_failure(
                            &self.senders,
                            None,
                            plugin_id,
                            &mut loading_indication,
                            format!("Failed to resolve plugin: {}", run.location),
                            Some(client_id),
                        );
                        return Ok((plugin_id, client_id));
                    },
                };
                let plugin_name = run.location.to_string();

                if plugin_config.is_shared() {
                    self.shared_plugin_ids.insert(plugin_id);
                }

                self.cached_resizes_for_pending_plugins
                    .insert(plugin_id, (size.rows, size.cols));
                self.cancel_token(plugin_id);

                // Clone for threaded contexts
                let plugin_executor = self.plugin_executor.clone();
                let senders = self.senders.clone();
                let yinz_cwd = cwd.unwrap_or_else(|| self.yinz_cwd.clone());
                let unloaded_while_loading = self.unloaded_while_loading.clone();

                let load_id = self.begin_plugin_load(plugin_id);
                let loading_context = LoadingContext::new(
                    self,
                    Some(yinz_cwd.clone()),
                    plugin_config,
                    plugin_id,
                    client_id,
                    tab_index,
                    size,
                )?;

                let _ =
                    senders.send_to_background_jobs(BackgroundJob::AnimatePluginLoading(plugin_id));
                let mut loading_indication = LoadingIndication::new(plugin_name.clone());

                let accepted = loading_context.execute_prepared(
                    self.plugin_executor.clone(),
                    senders.clone(),
                    load_id,
                    true,
                    move |loading_context,
                          senders,
                          plugin_map,
                          connected_clients,
                          plugin_cache: PluginCache,
                          engine: Engine| {
                        let mut plugin_map = plugin_map.lock().unwrap();
                        if load_was_cancelled(
                            &unloaded_while_loading,
                            plugin_id,
                            &senders,
                            &plugin_executor,
                        ) {
                            return;
                        }
                        match PluginLoader::new(
                            skip_cache,
                            loading_context,
                            senders.clone(),
                            engine.clone(),
                            plugin_cache.clone(),
                            &mut plugin_map,
                            connected_clients.clone(),
                        )
                        .start_plugin()
                        {
                            Ok(_) => {
                                let plugin_list = plugin_map.list_plugins();
                                handle_plugin_successful_loading(&senders, plugin_id, plugin_list);
                            },
                            Err(e) => handle_plugin_loading_failure(
                                &senders,
                                Some(load_id),
                                plugin_id,
                                &mut loading_indication,
                                e,
                                Some(client_id),
                            ),
                        }

                        let _ = senders.send_to_plugin(PluginInstruction::ApplyCachedEvents {
                            load_id,
                            plugin_ids: vec![plugin_id],
                            done_receiving_permissions: false,
                        });
                    },
                );
                if !accepted {
                    handle_plugin_loading_failure(
                        &self.senders,
                        Some(load_id),
                        plugin_id,
                        &mut LoadingIndication::new(plugin_id.to_string()),
                        "Plugin executor rejected loading",
                        None,
                    );
                }

                self.next_plugin_id += 1;
            },
            None => {
                self.next_plugin_id += 1;
                let mut loading_indication = LoadingIndication::new(format!("{}", plugin_id));
                handle_plugin_loading_failure(
                    &self.senders,
                    None,
                    plugin_id,
                    &mut loading_indication,
                    "Failed to resolve plugin alias",
                    None,
                );
            },
        }
        Ok((plugin_id, client_id))
    }
    pub fn unload_plugin(&mut self, pid: PluginId) -> Result<()> {
        self.unload_plugin_with_deadline(pid, None)
    }

    /// The token every instance of `plugin_id` checks; made on first use, so a load and the
    /// `LoadingContext` built for it share one. Removed by `unload_plugin`.
    fn cancel_token(&mut self, plugin_id: PluginId) -> Arc<AtomicBool> {
        self.cancel_tokens.entry(plugin_id).or_default().clone()
    }

    /// `unload_plugin`, with a bound on how long to wait for `plugin_map` when one is given.
    ///
    /// The loader holds that lock across the guest's `load()`, so at shutdown a plugin looping
    /// there without ever calling the host (which is what the cancel token needs) would hold the
    /// plugin thread forever. With a deadline the unload is skipped instead: the process is
    /// exiting, and what is lost is that plugin's `BeforeClose`.
    fn unload_plugin_with_deadline(
        &mut self,
        pid: PluginId,
        plugin_map_deadline: Option<Instant>,
    ) -> Result<()> {
        self.cancel_plugin_picks(pid);
        let _ = self
            .senders
            .send_to_screen(ScreenInstruction::NotificationPolicyUnavailable {
                plugin_id: pid,
                reason: "policy_unloaded".into(),
                disable: true,
            });
        info!("Bye from plugin {}", pid);
        // The screen may exit before plugins during shutdown; cleanup must still complete.
        let _ = self
            .senders
            .send_to_screen(ScreenInstruction::UnsubscribePaneOutput(pid, None));
        // The plugin's cancel token. Not set here: a plugin that is unloaded because it asked
        // to close itself is usually still inside the `update()` that asked, with more to do
        // (quit-confirmation closes itself and *then* sends `Quit`), and a token set now would
        // trap it between the two. At shutdown it is set already, by `cleanup`; otherwise the
        // watchdog armed below sets it only if the guest is still inside an entrypoint
        // `UNLOAD_GRACE` after the unload.
        let cancel = self.cancel_tokens.remove(&pid);
        let shutting_down = plugin_map_deadline.is_some();
        // The one exception: a plugin whose loader is still running. Its loader holds
        // `plugin_map` across the guest's `load()`, so if that `load()` is what never returns,
        // the lock below would wait on it forever — and after the `remove` above nothing else
        // could ever set its token. A plugin still loading has no `update()` to protect, so its
        // token can go first.
        let still_loading = self.loading_plugins.iter().any(|(p_id, _)| *p_id == pid);
        if let (Some(cancel), true) = (&cancel, still_loading) {
            cancel.store(true, Ordering::Relaxed);
        }
        let _ = self.senders.send_to_server(ServerInstruction::RebindKeys {
            client_id: ClientId::new(0),
            keys_to_rebind: vec![],
            keys_to_unbind: vec![],
            write_config_to_disk: false,
            plugin_id: Some(pid),
        });
        self.cancel_permission_requests(pid, None);
        self.plugin_roles.lock().unwrap().remove(&pid);
        self.independent_plugin_ids.remove(&pid);
        self.latest_plugin_errors.remove(&pid);
        self.plugin_callers.remove(&pid);
        self.pending_plugin_id_reloads.remove(&pid);
        if let Some(run) = self
            .run_plugin_of_loading_plugin_id(pid)
            .cloned()
            .or_else(|| self.run_plugin_of_plugin_id(pid))
        {
            self.pending_plugin_reloads.remove(&run);
        }
        self.shared_plugin_ids.remove(&pid);
        self.load_ids.remove(&pid);

        // Remove from plugin_map on main thread. A plugin whose loader has not run yet has
        // nothing to remove; it is marked instead, under the same lock, so the loader gives up
        // when its turn comes - see `unloaded_while_loading`.
        let plugins_to_cleanup: Vec<_> = {
            let Some(mut plugin_map) = lock_with_deadline(&self.plugin_map, plugin_map_deadline)
            else {
                log::error!(
                    "plugin {pid} could not be unloaded: its runtime is busy and the session is \
                     shutting down"
                );
                return Ok(());
            };
            let removed: Vec<_> = plugin_map.remove_plugins(pid).into_iter().collect();
            if removed.is_empty() && still_loading {
                self.unloaded_while_loading.lock().unwrap().insert(pid);
            }
            self.loading_plugins.retain(|(p_id, _)| *p_id != pid);
            // Keep resolved sources only while a running, loading, or retry identity owns
            // them. Configured declarations alone must not pin a stopped symlink's target.
            let retained_locations: HashSet<_> = plugin_map
                .list_plugins()
                .into_values()
                .chain(self.loading_plugins.iter().map(|(_, run)| run.clone()))
                .map(|run| self.normalize_plugin(run).location)
                .collect();
            self.resolved_plugin_locations
                .retain(|_, location| retained_locations.contains(location));
            removed
        };
        self.cached_events_for_pending_plugins.remove(&pid);
        self.cached_resizes_for_pending_plugins.remove(&pid);

        // Schedule cleanup on each plugin's pinned thread. Every (plugin, client)
        // pair gets its own job, and they all run on the plugin's one pinned thread
        // in this order; only the last one may unregister the plugin, because a job
        // submitted for an unregistered plugin is dropped — see
        // `PinnedExecutor::execute_plugin_unload`.
        let last_cleanup = plugins_to_cleanup.len().saturating_sub(1);
        if let (Some(cancel), false, false) =
            (&cancel, shutting_down, plugins_to_cleanup.is_empty())
        {
            let instances = plugins_to_cleanup
                .iter()
                .map(|(_, (running_plugin, _, _))| running_plugin.clone())
                .collect();
            arm_unload_watchdog(pid, cancel.clone(), instances);
        }
        for (index, ((plugin_id, client_id), (running_plugin, subscriptions, workers))) in
            plugins_to_cleanup.into_iter().enumerate()
        {
            // Unconditionally, rather than asking the plugin whether it intercepts: asking
            // takes its lock, and an instance still running on its executor thread holds that
            // lock for as long as it runs. The screen only drops an intercept that is this
            // plugin's, so a clear for one that never intercepted costs nothing.
            let _ = self
                .senders
                .send_to_screen(ScreenInstruction::ClearKeyPressesIntercepts(
                    plugin_id, client_id,
                ));
            // Clear any regex highlights this plugin registered across all panes
            let _ = self
                .senders
                .send_to_screen(ScreenInstruction::ClearAllPluginHighlights(plugin_id));

            // Drop its data store watches. They are keyed on a plugin id the server is free to
            // reuse, so leaving them would send a later plugin someone else's rows.
            self.data_access
                .plugin_data()
                .drop_watches_for_plugin(plugin_id);

            // Send worker exit messages
            for (_worker_name, worker_sender) in workers {
                drop(worker_sender.send(MessageToWorker::Exit));
            }

            let cleanup = move |senders: ThreadSenders,
                                _plugin_map: Arc<Mutex<PluginMap>>,
                                _connected_clients: Arc<Mutex<Vec<ClientId>>>,
                                _plugin_cache: PluginCache,
                                _engine: Engine| {
                close_plugin_instance(plugin_id, client_id, running_plugin, subscriptions, senders);
            };
            let accepted = if index == last_cleanup {
                self.plugin_executor
                    .execute_plugin_unload(plugin_id, cleanup)
            } else {
                self.plugin_executor
                    .execute_for_plugin_tagged(plugin_id, "unload", false, cleanup)
            };
            if !accepted {
                self.pending_pipes
                    .retire_plugin(plugin_id, "Plugin executor rejected retirement");
            }
        }

        self.pending_pipes
            .reject_cached(pid, None, "Plugin unloaded before pipe delivery");
        self.finish_pipe_requests();
        if let Some(plugin_map) = lock_with_deadline(&self.plugin_map, plugin_map_deadline) {
            let plugin_list = plugin_map.list_plugins();
            let _ = self
                .senders
                .send_to_background_jobs(BackgroundJob::ReportPluginList(plugin_list));
        }

        // The one place a queue slot is released. Every way a job can end - it reported, it was
        // cancelled, it closed itself, its pane was closed, it crashed - comes through here.
        let finished = self.queues.lock().unwrap().on_plugin_unloaded(pid);
        if let Some(finished) = finished {
            let queue = finished.queue.clone();
            self.report_finished_jobs(vec![finished]);
            self.pump_queue(&queue);
        }

        Ok(())
    }

    // --- work queues ---------------------------------------------------------------------------

    /// The configuration keys a job can read to learn what it is.
    pub const QUEUE_CONFIG_KEY: &'static str = "yinz_queue";
    pub const QUEUE_JOB_ID_CONFIG_KEY: &'static str = "yinz_queue_job_id";
    pub const QUEUE_ENQUEUER_CONFIG_KEY: &'static str = "yinz_queue_enqueuer_plugin_id";

    pub fn queue_configure(&mut self, queue: &str, concurrency: usize) {
        self.queues.lock().unwrap().configure(queue, concurrency);
        self.pump_queue(queue);
    }

    pub fn queue_cancel(&mut self, job_id: QueueJobId) -> Result<()> {
        let result = self.queues.lock().unwrap().cancel(job_id);
        match result {
            CancelResult::DroppedPending(finished) => {
                let queue = finished.queue.clone();
                self.report_finished_jobs(vec![finished]);
                self.broadcast_queue_changed(&queue);
                Ok(())
            },
            CancelResult::UnloadRunning(plugin_id) => self.close_job_plugin(plugin_id),
            CancelResult::NotFound => {
                log::warn!("cancel_queue_job: no job {job_id}");
                Ok(())
            },
        }
    }

    pub fn queue_finish_job(
        &mut self,
        plugin_id: PluginId,
        success: bool,
        payload: Option<String>,
    ) -> Result<()> {
        let outcome = if success {
            QueueJobOutcome::Succeeded
        } else {
            QueueJobOutcome::Failed
        };
        let recorded = self
            .queues
            .lock()
            .unwrap()
            .record_outcome(plugin_id, outcome, payload);
        if !recorded {
            log::warn!("finish_queue_job: plugin {plugin_id} is not a queued job, ignoring");
            return Ok(());
        }
        self.close_job_plugin(plugin_id)
    }

    /// A plugin failed to load or trapped. If it was a job, it is over: record why and unload
    /// it, so the slot frees and the enqueuer hears. Other failed instances retire while their
    /// metadata and error pane remain available for explicit reload.
    pub fn queue_plugin_failed(
        &mut self,
        plugin_id: PluginId,
        load_id: Option<u64>,
        deno_generation: Option<u64>,
        instance_id: Option<PluginInstanceId>,
        error: String,
        failed_to_load: bool,
    ) -> Result<()> {
        if load_id.is_some_and(|id| !self.load_is_current(plugin_id, id)) {
            return Ok(());
        }
        if !failed_to_load {
            let mut map = self.plugin_map.lock().unwrap();
            let instances = map.pipe_failure_instances(plugin_id, instance_id, deno_generation);
            if instances.is_empty() && (instance_id.is_some() || deno_generation.is_some()) {
                return Ok(());
            }
            drop(map);
            show_plugin_crash(plugin_id, error.clone(), &self.senders);
            for (id, instance) in instances {
                let accepted = self.plugin_executor.execute_for_plugin_tagged(
                    plugin_id,
                    "failed_dispose",
                    false,
                    move |_, _, _, _, _| instance.lock().unwrap().dispose(),
                );
                if !accepted {
                    self.pending_pipes.retire(plugin_id, id, &error);
                }
            }
        } else {
            self.pending_pipes.reject_cached(plugin_id, load_id, &error);
        }
        self.cancel_plugin_picks(plugin_id);
        if !failed_to_load && !self.loading_plugins.iter().any(|(id, _)| *id == plugin_id) {
            let instances = self.plugin_map.lock().unwrap().pipe_instances();
            let shared = self.plugin_is_shared(&plugin_id);
            if let Some(cache) = self.cached_events_for_pending_plugins.get_mut(&plugin_id) {
                cache.retain(|message| {
                    if let EventOrPipeMessage::PipeMessage(client, _, request, load) = message {
                        if !instances.iter().any(|((id, owner, _, _), _)| {
                            Self::message_is_directed_at_plugin(
                                Some(plugin_id),
                                *client,
                                id,
                                owner,
                                shared,
                            )
                        }) {
                            if let Some(request) = request {
                                self.pending_pipes.finish_cached(
                                    *request,
                                    plugin_id,
                                    *load,
                                    Err(error.clone()),
                                );
                            }
                            return false;
                        }
                    }
                    true
                });
            }
        }
        self.senders
            .send_to_screen(ScreenInstruction::NotificationPolicyUnavailable {
                plugin_id,
                reason: if failed_to_load {
                    "policy_load_failed"
                } else {
                    "policy_crashed"
                }
                .into(),
                disable: true,
            })
            .context("resolve notifications after policy failure")
            .non_fatal();
        if let Some(identity) = self.plugin_map.lock().unwrap().log_identity(plugin_id) {
            for line in error.lines() {
                log::error!("{} {}", identity, line);
            }
        }
        self.cancel_permission_requests(plugin_id, instance_id);
        self.latest_plugin_errors.insert(plugin_id, error.clone());
        self.finish_pipe_requests();
        let recorded = self.queues.lock().unwrap().record_outcome(
            plugin_id,
            QueueJobOutcome::Failed,
            Some(error),
        );
        if !recorded {
            if failed_to_load {
                let run_plugin = self.run_plugin_of_loading_plugin_id(plugin_id).cloned();
                self.cached_events_for_pending_plugins.remove(&plugin_id);
                self.cached_resizes_for_pending_plugins.remove(&plugin_id);
                self.loading_plugins.retain(|(id, _)| *id != plugin_id);
                self.plugin_ids_waiting_for_permission_request
                    .remove(&plugin_id);
                if self.pending_plugin_id_reloads.remove(&plugin_id) {
                    self.reload_plugin_with_id(plugin_id)?;
                }
                if let Some(run_plugin) = run_plugin {
                    if self.pending_plugin_reloads.remove(&run_plugin) {
                        self.reload_plugin(&run_plugin)?;
                    }
                }
            }
            return Ok(());
        }
        if failed_to_load {
            self.forget_failed_load(plugin_id);
        }
        self.close_job_plugin(plugin_id)
    }

    /// A job whose *load* failed, on its way to being unloaded. Its loader has already run and
    /// given up, so there is nothing left to cancel: dropping it from `loading_plugins` here is
    /// what stops the unload below from leaving behind a cancellation mark nobody will consume
    /// (the loader's trailing `ApplyCachedEvents`, which usually does this, has not arrived yet).
    /// The load registered the plugin with the executor and never unloaded it, so that is ours
    /// to undo too.
    fn forget_failed_load(&mut self, plugin_id: PluginId) {
        self.loading_plugins.retain(|(p_id, _)| *p_id != plugin_id);
        // a no-op on the plugin's own thread, so the unregister lands after the loader closure
        self.plugin_executor
            .execute_plugin_unload(plugin_id, |_, _, _, _, _| {});
    }

    /// The `close_self` pair, done host-side: drop the suppressed pane, unload the plugin. The
    /// unload runs the queue hook, which reports the job and starts the next one.
    fn close_job_plugin(&mut self, plugin_id: PluginId) -> Result<()> {
        self.senders
            .send_to_screen(ScreenInstruction::ClosePane(
                PaneId::Plugin(plugin_id),
                None,
                None,
                None,
            ))
            .with_context(|| format!("failed to close the pane of job plugin {plugin_id}"))
            .non_fatal();
        self.unload_plugin(plugin_id)
            .with_context(|| format!("failed to unload job plugin {plugin_id}"))
    }

    /// Start jobs on `queue` until it is at its limit or empty, one at a time so that a launch
    /// that fails frees its slot for the next before we decide whether there is room.
    pub fn pump_queue(&mut self, queue: &str) {
        if self.queues.lock().unwrap().is_shut_down() {
            return;
        }
        let mut finished = vec![];
        loop {
            let job = self.queues.lock().unwrap().pop_launchable(queue);
            let Some(job) = job else {
                break;
            };
            match self.launch_job(&job) {
                Ok(plugin_id) => {
                    let started = Event::QueueJobStarted {
                        queue: job.queue.clone(),
                        job_id: job.job_id,
                        plugin_id,
                        context: job.context.clone(),
                    };
                    let enqueuer = job.enqueuer;
                    let deno_generation = job.deno_generation;
                    self.queues.lock().unwrap().on_job_launched(job, plugin_id);
                    self.send_event_to(enqueuer, deno_generation, started);
                },
                Err(e) => {
                    finished.push(
                        self.queues
                            .lock()
                            .unwrap()
                            .on_launch_failed(job, format!("{e:#}")),
                    );
                },
            }
        }
        self.report_finished_jobs(finished);
        self.broadcast_queue_changed(queue);
    }

    /// Load the job's plugin as a fresh headless instance, with its queue identity in its
    /// configuration. Bypasses `get_or_load_plugins` deliberately: that dedupes on
    /// `(location, configuration)`, and two jobs of the same plugin must be two instances.
    fn launch_job(&mut self, job: &QueuedJob) -> Result<PluginId> {
        let mut run_plugin_or_alias = job.run_plugin_or_alias.clone();
        run_plugin_or_alias.populate_run_plugin_if_needed(&self.plugin_aliases.lock().unwrap());
        let mut run_plugin = run_plugin_or_alias.get_run_plugin().ok_or_else(|| {
            anyhow!(
                "{} could not be resolved",
                job.run_plugin_or_alias.location_string()
            )
        })?;
        run_plugin
            .configuration
            .insert(Self::QUEUE_CONFIG_KEY, job.queue.clone());
        run_plugin
            .configuration
            .insert(Self::QUEUE_JOB_ID_CONFIG_KEY, job.job_id.to_string());
        run_plugin
            .configuration
            .insert(Self::QUEUE_ENQUEUER_CONFIG_KEY, job.enqueuer.0.to_string());
        // A job is session work, not per-client UI: one instance, however many clients are
        // attached now or later. Without this the loader would instantiate it once per client,
        // each copy running the same job. Set last so the enqueuer's configuration cannot undo it.
        run_plugin.configuration.insert("shared", "true".to_owned());
        let pending_id = PluginId::new(self.next_plugin_id);
        self.plugin_roles.lock().unwrap().insert(pending_id, true);
        let (plugin_id, client_id) = self
            .load_plugin(
                &Some(run_plugin.clone()),
                None,
                Size::default(),
                job.cwd.clone(),
                false,
                Some(job.enqueuer.1),
            )
            .inspect_err(|_| {
                self.plugin_roles.lock().unwrap().remove(&pending_id);
            })?;
        let _ = self
            .senders
            .send_to_screen(super::add_suppressed_plugin_pane(
                RunPluginOrAlias::RunPlugin(run_plugin),
                plugin_id,
                job.cwd.clone(),
                client_id,
            ));
        Ok(plugin_id)
    }

    fn report_finished_jobs(&self, finished: Vec<FinishedJob>) {
        for job in finished {
            let event = Event::QueueJobFinished {
                queue: job.queue,
                job_id: job.job_id,
                outcome: job.outcome,
                payload: job.payload,
                context: job.context,
            };
            self.send_event_to(job.enqueuer, job.deno_generation, event);
        }
    }

    fn broadcast_queue_changed(&self, queue: &str) {
        let snapshot = self.queues.lock().unwrap().queue_snapshot(queue);
        let Some(snapshot) = snapshot else {
            return;
        };
        let event = Event::QueueChanged {
            queue: snapshot.queue,
            concurrency: snapshot.concurrency,
            pending: snapshot.pending,
            running: snapshot.running,
        };
        let _ = self
            .senders
            .send_to_plugin(PluginInstruction::Update(vec![(None, None, event)]));
    }

    /// Queue events go back through the bus like a `RunCommandResult` does, rather than being
    /// applied here: this can run from inside `unload_plugin`, and the update path takes the
    /// same locks.
    fn send_event_to(
        &self,
        (plugin_id, client_id): (PluginId, ClientId),
        deno_generation: Option<u64>,
        event: Event,
    ) {
        let _ = self
            .senders
            .send_to_plugin(PluginInstruction::for_plugin_event(
                plugin_id,
                client_id,
                deno_generation,
                event,
            ));
    }
    pub fn reload_plugin_with_id(&mut self, plugin_id: PluginId) -> Result<()> {
        let picker = self.picks.contains_key(&plugin_id);
        self.cancel_plugin_picks(plugin_id);
        if picker {
            return Ok(());
        }
        self.cancel_permission_requests(plugin_id, None);
        self.senders
            .send_to_screen(ScreenInstruction::NotificationPolicyUnavailable {
                plugin_id,
                reason: "policy_reloaded".into(),
                disable: false,
            })
            .context("invalidate notification requests on reload")
            .non_fatal();
        // Configuration is cached outside the guest mutex. Give the replacement its own
        // uncancelled token, and the old instance the grace an unload gets: a token set at
        // once also stops a call that was about to return, and an instance stopped mid-call
        // is not given `BeforeClose`.
        if self
            .plugin_config_of_plugin_id(plugin_id)
            .is_some_and(|config| config.runtime() == yinz_config::plugins::PluginRuntime::Deno)
        {
            let cancel = self.cancel_token(plugin_id);
            self.cancel_tokens
                .insert(plugin_id, Arc::new(AtomicBool::new(false)));
            let instances = self
                .plugin_map
                .lock()
                .unwrap()
                .running_plugins_and_subscriptions()
                .into_iter()
                .filter(|(id, ..)| *id == plugin_id)
                .map(|(_, _, running_plugin, _)| Arc::downgrade(&running_plugin))
                .collect();
            arm_reload_watchdog(plugin_id, cancel, instances);
        }
        let Some(run_plugin) = self.run_plugin_of_plugin_id(plugin_id) else {
            log::error!("Failed to find plugin with id: {}", plugin_id);
            return Ok(());
        };

        // A failure already queued by the old generation must not finish a queue job or
        // paint over this reload, even before the executor installs the replacement.
        self.plugin_map
            .lock()
            .unwrap()
            .invalidate_deno_generations(plugin_id);

        self.plugin_map.lock().unwrap().retire_instances(plugin_id);
        self.plugin_ids_waiting_for_permission_request
            .remove(&plugin_id);
        self.pending_pipes
            .reject_cached(plugin_id, None, "Plugin reloaded before pipe delivery");
        self.cached_events_for_pending_plugins.remove(&plugin_id);
        self.finish_pipe_requests();
        let (rows, columns) = self.size_of_plugin_id(plugin_id).unwrap_or((0, 0));
        self.cached_events_for_pending_plugins
            .entry(plugin_id)
            .or_default();
        self.cached_resizes_for_pending_plugins
            .insert(plugin_id, (rows, columns));

        let mut loading_indication = LoadingIndication::new(run_plugin.location.to_string());
        self.start_plugin_loading_indication(&[plugin_id], &loading_indication);
        self.loading_plugins.insert((plugin_id, run_plugin.clone()));
        // Wasm keep their token; Deno's replacement token was installed above.
        self.cancel_token(plugin_id);

        let plugin_executor = self.plugin_executor.clone();

        let Some(first_client_id) = self.get_first_client_id().or_else(|| {
            self.plugin_map
                .lock()
                .unwrap()
                .reload_metadata(plugin_id)
                .map(|metadata| metadata.owner_client_id)
        }) else {
            log::error!("No instance owner, cannot reload plugin.");
            return Ok(());
        };
        let Some(mut plugin_config) = self.plugin_config_of_plugin_id(plugin_id) else {
            log::error!("Could not find running plugin with id: {}", plugin_id);
            return Ok(());
        };
        if let RunPluginLocation::Git(source) = &plugin_config.location {
            plugin_config.path = source.entrypoint.clone();
            plugin_config.repository_root = None;
            plugin_config.installed_commit = None;
        }
        let tab_index = self.tab_index_of_plugin_id(plugin_id);
        let Some(size) = self.size_of_plugin_id(plugin_id) else {
            log::error!(
                "Could not find size of running plugin with id: {}",
                plugin_id
            );
            return Ok(());
        };
        let size = Size {
            rows: size.0,
            cols: size.1,
        };

        let cwd = self.cwd_of_plugin_id(plugin_id);

        let load_id = self.begin_plugin_load(plugin_id);
        let loading_context = LoadingContext::new(
            self,
            cwd,
            plugin_config,
            plugin_id,
            first_client_id,
            tab_index,
            size,
        )?;

        let accepted = loading_context.execute_prepared(
            plugin_executor,
            self.senders.clone(),
            load_id,
            false,
            move |mut loading_context,
                  senders,
                  plugin_map,
                  connected_clients,
                  plugin_cache,
                  engine| {
                let skip_cache = true; // we want to explicitly reload the plugin
                let mut plugin_map = plugin_map.lock().unwrap();
                // A failed instance retains its identity; explicit unload removes it.
                if plugin_map.run_plugin_of_plugin_id(plugin_id).is_none() {
                    return;
                }
                // The instance about to be replaced gets the same last word an unloaded one
                // does, so a plugin that persists in `BeforeClose` survives its own reload.
                for (id, client_id, running_plugin, subscriptions) in
                    plugin_map.running_plugins_and_subscriptions()
                {
                    if id != plugin_id {
                        continue;
                    }
                    let wants_before_close = subscriptions
                        .lock()
                        .unwrap()
                        .contains(&EventType::BeforeClose);
                    let old_instance = running_plugin.clone();
                    let mut running_plugin = running_plugin.lock().unwrap();
                    let cleanup_reason = if running_plugin.is_disposed() {
                        "disposed"
                    } else if running_plugin.failed {
                        "failed"
                    } else if !wants_before_close {
                        "unsubscribed"
                    } else if running_plugin.env().cancel_tripped.load(Ordering::Acquire) {
                        "interrupted"
                    } else {
                        "deliver"
                    };
                    log::info!(
                        "reload cleanup: pid={} plugin={plugin_id} client={client_id} generation={:?} subscribed={wants_before_close} disposed={} failed={} cancel={} tripped={} reason={cleanup_reason}",
                        std::process::id(), running_plugin.env().deno_generation,
                        running_plugin.is_disposed(), running_plugin.failed,
                        running_plugin.env().cancel.load(Ordering::Acquire),
                        running_plugin.env().cancel_tripped.load(Ordering::Acquire),
                    );
                    if client_id == first_client_id
                        || running_plugin.env().plugin_config.is_shared()
                    {
                        if let Some(directory) = &running_plugin.env().temporary_data_dir {
                            // Retain the reload owner's data, independent of instance iteration order.
                            loading_context.plugin_own_data_dir = directory.path().to_path_buf();
                            loading_context.temporary_data_dir = directory.clone();
                        }
                    }
                    if wants_before_close && !running_plugin.is_disposed() && !running_plugin.failed
                    {
                        // As in the unload job: holding the lock means the guest is not
                        // running, and the new instance shares this token, so a set one
                        // would trap `BeforeClose` here and the new `load()` after it.
                        // And as there, an instance that was stopped mid-call is not
                        // called again — see `PluginEnv::cancel_tripped`.
                        let was_stopped_mid_call = running_plugin
                            .env()
                            .cancel_tripped
                            .load(std::sync::atomic::Ordering::Relaxed);
                        if was_stopped_mid_call {
                            log::warn!(
                                "plugin {plugin_id} was cancelled mid-call, so its BeforeClose \
                                 is skipped before its reload"
                            );
                        } else {
                            running_plugin.env().cancel.store(false, Ordering::Release);
                            if running_plugin.env().deno_generation.is_some() {
                                arm_unload_watchdog(
                                    plugin_id,
                                    running_plugin.env().cancel.clone(),
                                    vec![old_instance],
                                );
                            }
                            if let Err(e) = apply_before_close_event_to_plugin(
                                plugin_id,
                                client_id,
                                &mut running_plugin,
                                senders.clone(),
                            ) {
                                log::error!("{:?}", e);
                            }
                        }
                    }
                    running_plugin.dispose();
                    drop(running_plugin);
                    plugin_map.remove_instance(plugin_id, client_id);
                }
                // A replacement must opt in again, including when BeforeClose resubscribed.
                let _ = senders
                    .send_to_screen(ScreenInstruction::UnsubscribePaneOutput(plugin_id, None));
                match PluginLoader::new(
                    skip_cache,
                    loading_context,
                    senders.clone(),
                    engine.clone(),
                    plugin_cache.clone(),
                    &mut plugin_map,
                    connected_clients.clone(),
                )
                .start_plugin()
                {
                    Ok(_) => {
                        if let Some(location) = plugin_map.update_reloaded_file_location(plugin_id)
                        {
                            senders
                                .send_to_screen(ScreenInstruction::UpdatePluginLocation(
                                    plugin_id, location,
                                ))
                                .context("update reloaded plugin pane identity")
                                .non_fatal();
                        }
                        let plugin_list = plugin_map.list_plugins();
                        handle_plugin_successful_loading(&senders, plugin_id, plugin_list);
                    },
                    Err(e) => handle_plugin_loading_failure(
                        &senders,
                        Some(load_id),
                        plugin_id,
                        &mut loading_indication,
                        e,
                        Some(first_client_id),
                    ),
                }
                let _ = senders.send_to_plugin(PluginInstruction::ApplyCachedEvents {
                    load_id,
                    plugin_ids: vec![plugin_id],
                    done_receiving_permissions: false,
                });
            },
        );
        if !accepted {
            self.pending_pipes
                .retire_plugin(plugin_id, "Plugin executor rejected replacement");
            self.queue_plugin_failed(
                plugin_id,
                Some(load_id),
                None,
                None,
                "Plugin executor rejected loading".into(),
                true,
            )?;
        }
        Ok(())
    }
    pub fn reload_plugin(&mut self, run_plugin: &RunPlugin) -> Result<()> {
        self.remember_plugin_location(run_plugin);
        if let Some(id) =
            self.plugin_id_of_loading_plugin(&run_plugin.location, &run_plugin.configuration)
        {
            if let Some(loading) = self.run_plugin_of_loading_plugin_id(id) {
                self.pending_plugin_reloads.insert(loading.clone());
            }
            return Ok(());
        }

        let plugin_ids = self
            .all_plugin_ids_for_plugin_location(&run_plugin.location, &run_plugin.configuration)?;
        for plugin_id in &plugin_ids {
            self.reload_plugin_with_id(*plugin_id)?;
        }
        Ok(())
    }
    pub fn add_client(&mut self, client_id: ClientId) -> Result<()> {
        let started_at = std::time::Instant::now();
        log::info!(
            "attach trace: WasmBridge::add_client client_id={client_id} connected-client check begin"
        );
        if self.client_is_connected(&client_id) {
            log::info!(
                "attach trace: WasmBridge::add_client client_id={client_id} already connected"
            );
            return Ok(());
        }

        self.connected_clients.lock().unwrap().push(client_id);
        let mut new_plugins = HashSet::new();
        let client_cancel = {
            let mut cancellations = self.client_cancellations.lock().unwrap();
            let cancel = cancellations.entry(client_id).or_default();
            if cancel.load(Ordering::Acquire) {
                *cancel = Arc::default();
            }
            cancel.clone()
        };
        log::info!(
            "attach trace: WasmBridge::add_client client_id={client_id} waiting for plugin_map lock"
        );
        let existing_plugin_ids = self.plugin_map.lock().unwrap().plugin_ids();
        log::info!(
            "attach trace: WasmBridge::add_client client_id={client_id} plugin_map read complete plugins={} after {:?}",
            existing_plugin_ids.len(),
            started_at.elapsed()
        );
        for plugin_id in existing_plugin_ids {
            if self.plugin_is_shared(&plugin_id) {
                // a shared plugin has one instance for the whole session - we don't load another
                // one for this client, we just make sure it's owned by a connected client
                self.rekey_shared_plugin_if_owner_gone(plugin_id, client_id);
                continue;
            }
            new_plugins.insert(plugin_id);
        }
        for plugin_id in new_plugins {
            log::info!(
                "attach trace: WasmBridge::add_client client_id={client_id} plugin_id={plugin_id} instance check begin"
            );
            // a plugin loaded before this client attached (eg. a background plugin loaded on
            // startup, keyed under the initiating client's id) already has an instance for this
            // client - loading another would instantiate it twice and drop the first
            if self
                .plugin_map
                .lock()
                .unwrap()
                .contains(plugin_id, client_id)
            {
                log::info!(
                    "attach trace: WasmBridge::add_client client_id={client_id} plugin_id={plugin_id} already instantiated"
                );
                continue;
            }
            let Some(run_plugin) = self.run_plugin_of_plugin_id(plugin_id) else {
                log::error!("Failed to find plugin with id: {}", plugin_id);
                return Ok(());
            };

            let (rows, columns) = self.size_of_plugin_id(plugin_id).unwrap_or((0, 0));
            // Started, not reset: a plugin that is already pending - still loading for its
            // first client, or waiting on its permission prompt - has events cached that are
            // owed to it, and a client attaching in that window used to wipe them, so a
            // lazily-loaded hook lost the very firing that loaded it. The cache now holds until
            // the plugin is ready, and is then applied to every client's instance.
            self.cached_events_for_pending_plugins
                .entry(plugin_id)
                .or_default();
            self.cached_resizes_for_pending_plugins
                .insert(plugin_id, (rows, columns));

            let loading_indication = LoadingIndication::new(run_plugin.location.to_string());
            self.start_plugin_loading_indication(&[plugin_id], &loading_indication);
            self.loading_plugins.insert((plugin_id, run_plugin.clone()));

            let plugin_executor = self.plugin_executor.clone();

            let Some(plugin_config) = self.plugin_config_of_plugin_id(plugin_id) else {
                log::error!("Could not find running plugin with id: {}", plugin_id);
                return Ok(());
            };
            let tab_index = self.tab_index_of_plugin_id(plugin_id);
            let Some(size) = self.size_of_plugin_id(plugin_id) else {
                log::error!(
                    "Could not find size of running plugin with id: {}",
                    plugin_id
                );
                return Ok(());
            };
            let size = Size {
                rows: size.0,
                cols: size.1,
            };

            let cwd = self.cwd_of_plugin_id(plugin_id);

            // So the context below shares the plugin's token rather than getting one nobody
            // sets; a no-op when the plugin is registered, as it normally is here.
            self.cancel_token(plugin_id);
            let load_id = self.begin_plugin_load(plugin_id);
            let mut loading_context = LoadingContext::new(
                self,
                cwd,
                plugin_config,
                plugin_id,
                client_id,
                tab_index,
                size,
            )?;
            // This attachment is not connected yet, but must share the token remove_client sets.
            loading_context.client_cancel = client_cancel.clone();

            log::info!(
                "attach trace: WasmBridge::add_client client_id={client_id} plugin_id={plugin_id} executor enqueue begin"
            );
            let accepted = loading_context.execute_prepared(
                plugin_executor, self.senders.clone(), load_id, false,
                move |loading_context, senders, plugin_map, connected_clients, plugin_cache, engine| {
                    let skip_cache = false;
                    let load_started_at = std::time::Instant::now();
                    log::info!(
                        "attach trace: plugin executor client_id={client_id} plugin_id={plugin_id} waiting for plugin_map lock"
                    );
                    let plugin_map_for_cleanup = plugin_map.clone();
                    let mut plugin_map = plugin_map.lock().unwrap();
                    if !plugin_map.plugin_ids().contains(&plugin_id) { return; }
                    log::info!(
                        "attach trace: plugin executor client_id={client_id} plugin_id={plugin_id} acquired plugin_map lock after {:?}; plugin load begin",
                        load_started_at.elapsed()
                    );
                    match PluginLoader::new(
                        skip_cache,
                        loading_context,
                        senders.clone(),
                        engine.clone(),
                        plugin_cache.clone(),
                        &mut plugin_map,
                        connected_clients.clone(),
                    )
                    .without_connected_clients()
                    .start_plugin()
                    {
                        Ok(_) => {
                            log::info!(
                                "attach trace: plugin executor client_id={client_id} plugin_id={plugin_id} plugin load complete after {:?}",
                                load_started_at.elapsed()
                            );
                            let _ = senders
                                .send_to_screen(ScreenInstruction::RequestStateUpdateForPlugins);
                            let _ = senders.send_to_background_jobs(
                                BackgroundJob::StopPluginLoadingAnimation(plugin_id),
                            );
                            let _ = senders.send_to_plugin(PluginInstruction::ApplyCachedEvents {
                                        load_id,
                                plugin_ids: vec![plugin_id],
                                done_receiving_permissions: false,
                            });
                        },
                        Err(e) => {
                            log::info!(
                                "attach trace: plugin executor client_id={client_id} plugin_id={plugin_id} plugin load failed after {:?}",
                                load_started_at.elapsed()
                            );
                            log::error!("Failed to load plugin for new client: {}", e);
                            let _ = senders.send_to_plugin(PluginInstruction::ApplyCachedEvents {
                                        load_id,
                                plugin_ids: vec![plugin_id], done_receiving_permissions: false,
                            });
                        },
                    }
                    drop(plugin_map);
                    cleanup_disconnected_instances(plugin_id, &connected_clients, &plugin_map_for_cleanup, senders);

                },
            );
            if !accepted {
                self.queue_plugin_failed(
                    plugin_id,
                    Some(load_id),
                    None,
                    None,
                    "Plugin executor rejected loading".into(),
                    true,
                )?;
            }
            log::info!(
                "attach trace: WasmBridge::add_client client_id={client_id} plugin_id={plugin_id} executor enqueue complete after {:?}",
                started_at.elapsed()
            );
        }
        log::info!(
            "attach trace: WasmBridge::add_client client_id={client_id} complete after {:?}",
            started_at.elapsed()
        );
        Ok(())
    }
    pub fn resize_plugin(
        &mut self,
        pid: PluginId,
        new_columns: usize,
        new_rows: usize,
        shutdown_sender: Sender<()>,
    ) -> Result<()> {
        let err_context = move || format!("failed to resize plugin {pid}");

        let plugins_to_resize = if self.cached_resizes_for_pending_plugins.contains_key(&pid) {
            vec![]
        } else {
            self.plugin_map
                .lock()
                .map_err(|_| anyhow!("plugin map lock poisoned"))
                .with_context(err_context)?
                .resize_targets(pid)
        };
        if let Some(current_size) = self.cached_resizes_for_pending_plugins.get_mut(&pid) {
            *current_size = (new_rows, new_columns);
        }
        for (client_id, running_plugin, slot) in plugins_to_resize {
            // Asked here, on the plugin thread, before the executor gets to any queued job: it
            // is how a queued resize learns a newer size is wanted. A job already queued will
            // draw it, so none is queued behind it. See `ResizeSlot`.
            if !slot.request(new_rows, new_columns) {
                continue;
            }
            let shared = self.plugin_is_shared(&pid);
            let queued_slot = slot.clone();
            // Execute directly on pinned thread (no async I/O needed for resize/render)
            let accepted = self
                .plugin_executor
                .execute_for_plugin_tagged(pid, "resize", false, {
                    let _s = shutdown_sender.clone();
                    move |senders, _plugin_map, connected_clients, _plugin_cache, _engine| {
                        // Before the guest lock: a resize some earlier render already drew
                        // must not wait for, or cost, another. Begun before any return, so a
                        // request from here on queues the job that draws it.
                        if !slot.begin_job() {
                            return;
                        }
                        // A poisoned instance is a guest that panicked: nothing to draw.
                        let Ok(mut running_plugin) = running_plugin.lock() else {
                            return;
                        };
                        // And again with it: a callback that held it can have drawn the size.
                        if running_plugin.is_unavailable() || !slot.resize_due() {
                            return;
                        }
                        // This executor preserves request order. Allocate the id
                        // here so the plugin thread can answer synchronous guest
                        // queries while a previous callback owns the guest lock.
                        let event_id = running_plugin.next_event_id(AtomicEvent::Resize);
                        let _s = _s; // guard to allow the task to complete before cleanup/shutdown
                        if running_plugin.apply_event_id(AtomicEvent::Resize, event_id) {
                            slot.note_resize();
                            // Read now rather than when this job was queued: the job that draws
                            // is the oldest one queued, and a client can have attached or left
                            // since. A plain list, so a poisoned lock still holds a usable one.
                            let render_clients = if shared {
                                connected_clients
                                    .lock()
                                    .unwrap_or_else(PoisonError::into_inner)
                                    .clone()
                            } else {
                                vec![client_id]
                            };
                            match render_plugin_for_clients(
                                pid,
                                &mut running_plugin,
                                &render_clients,
                            )
                            .with_context(err_context)
                            {
                                Ok(plugin_render_assets) => {
                                    report_pipe_changes(
                                        &mut running_plugin,
                                        HashMap::new(),
                                        None,
                                        &senders,
                                    );
                                    // The screen may already have exited during shutdown.
                                    // Release the guest before publishing, as event and
                                    // pipe renders do, so a send cannot poison its lock.
                                    drop(running_plugin);
                                    senders
                                        .send_to_screen(ScreenInstruction::PluginBytes(
                                            plugin_render_assets,
                                        ))
                                        .with_context(err_context)
                                        .non_fatal();
                                },
                                Err(error) => {
                                    running_plugin.mark_failed();
                                    handle_plugin_crash(
                                        pid,
                                        running_plugin.env().deno_generation,
                                        Some(running_plugin.instance_id),
                                        format!("{error:#}"),
                                        senders,
                                    );
                                },
                            }
                        }
                    }
                });
            // A refused job never begins, so it must not stay counted as queued: every later
            // request would wait on it and the instance would never resize again.
            if !accepted {
                queued_slot.cancel_job();
            }
        }
        Ok(())
    }
    pub(crate) fn resolve_agent_event_projects(
        &self,
        version: u64,
        agents: Vec<yinz_types::agent::control::AgentStatus>,
        projects: Vec<String>,
    ) {
        let data = self.data_access.clone();
        let senders = self.senders.clone();
        std::thread::spawn(move || {
            let result =
                crate::yinz_exports::agent_event_projects(agents, projects, &data, &senders);
            senders
                .send_to_screen(ScreenInstruction::AgentEventProjects(version, result))
                .non_fatal();
        });
    }

    /// A `yinz events` client asking for `event_types` by name. A name that is not a
    /// streamable event refuses the whole subscription, told to the client as an error, so a
    /// typo is an exit rather than a silent stream of everything else.
    pub fn subscribe_cli_client_to_events(
        &mut self,
        client_id: ClientId,
        event_types: Vec<String>,
    ) {
        let mut wanted = HashSet::new();
        for name in &event_types {
            match EventType::from_cli_name(name) {
                Some(event_type) => {
                    wanted.insert(event_type);
                },
                None => {
                    let _ = self.senders.send_to_server(ServerInstruction::LogError(
                        vec![format!(
                            "Unknown or unstreamable event type '{}'. Streamable events: {}",
                            name,
                            EventType::cli_streamable_names().join(", ")
                        )],
                        client_id,
                        None,
                    ));
                    return;
                },
            }
        }
        self.cli_event_subscribers.insert(client_id, wanted);
        // Sent for every subscriber, not just the first: the screen answers it with a state
        // report, which is the snapshot a new subscriber starts from. It also has the screen
        // report pane and tab state when no plugin wants it, addressed to
        // `CLI_EVENTS_ONLY_TARGET`.
        let _ = self
            .senders
            .send_to_screen(ScreenInstruction::SetCliEventSubscribersPresent(true));
    }

    /// Hand every streamable event in `updates` to the `yinz events` clients that want it.
    ///
    /// A state report — `PaneUpdate`, `TabUpdate`, `ModeUpdate`, `SessionUpdate` — arrives here
    /// as one copy per (plugin, client) it is addressed to, all alike but for the addressee, and
    /// a batch merged on the plugin thread can carry several generations of one. Within a run of
    /// consecutive reports only the newest of each kind is forwarded, since each describes the
    /// whole state; a non-report event ends the run, so a key is never forwarded ahead of the
    /// report that described the mode it was pressed in. Everything else is forwarded as is.
    fn forward_events_to_cli_subscribers(
        &self,
        updates: &[(Option<PluginId>, Option<ClientId>, Event)],
    ) {
        if self.cli_event_subscribers.is_empty() {
            return;
        }
        let is_state_report = |event_type: &EventType| {
            matches!(
                event_type,
                EventType::PaneUpdate
                    | EventType::TabUpdate
                    | EventType::ModeUpdate
                    | EventType::SessionUpdate
            )
        };
        // the newest index of each kind of report in the current run, in order of first
        // appearance
        let mut pending: Vec<(EventType, usize)> = vec![];
        let flush = |pending: &mut Vec<(EventType, usize)>| {
            for (event_type, index) in pending.drain(..) {
                self.forward_event_to_cli_subscribers(event_type, &updates[index].2);
            }
        };
        for (index, (_, _, event)) in updates.iter().enumerate() {
            let event_type = EventType::from(event);
            if !event_type.is_cli_streamable() {
                continue;
            }
            if is_state_report(&event_type) {
                match pending.iter_mut().find(|(kind, _)| *kind == event_type) {
                    Some(slot) => slot.1 = index,
                    None => pending.push((event_type, index)),
                }
            } else {
                flush(&mut pending);
                self.forward_event_to_cli_subscribers(event_type, event);
            }
        }
        flush(&mut pending);
    }

    fn forward_event_to_cli_subscribers(&self, event_type: EventType, event: &Event) {
        let event_json = match serde_json::to_string(event) {
            Ok(event_json) => event_json,
            Err(e) => {
                log::error!(
                    "failed to serialize {} for a cli subscriber: {}",
                    event_type,
                    e
                );
                return;
            },
        };
        for (client_id, wanted) in &self.cli_event_subscribers {
            if wanted.is_empty() || wanted.contains(&event_type) {
                let _ = self
                    .senders
                    .send_to_server(ServerInstruction::EventUpdateToClient(
                        *client_id,
                        event_type.to_string(),
                        event_json.clone(),
                    ));
            }
        }
    }

    #[cfg(test)]
    pub fn deno_event(
        &mut self,
        plugin_id: PluginId,
        generation: u64,
        events: Vec<(ClientId, Event)>,
        shutdown_sender: Sender<()>,
    ) -> Result<()> {
        self.deno_replies(
            plugin_id,
            generation,
            events
                .into_iter()
                .map(|(client, event)| (client, event, None))
                .collect(),
            shutdown_sender,
        )
    }

    pub fn deno_replies(
        &mut self,
        plugin_id: PluginId,
        generation: u64,
        events: Vec<(ClientId, Event, Option<yinz_bus::thread_bus::DenoReplyId>)>,
        shutdown_sender: Sender<()>,
    ) -> Result<()> {
        // Pane lifecycle events still describe the session after their plugin reloads.
        for (_, event, _) in &events {
            let event_type = EventType::from(event);
            if !self.cli_event_subscribers.is_empty() && event_type.is_cli_streamable() {
                self.forward_event_to_cli_subscribers(event_type, event);
            }
        }
        // Loading can hold the map while guest code runs. The plugin thread must
        // remain free to process the cancellation that interrupts that code.
        let (batch_label, batch_has_input) =
            summarize_batch_for_stats(events.iter().map(|(_, event, _)| event));
        let messages: Vec<_> = events
            .into_iter()
            .map(|(client, event, reply)| {
                EventOrPipeMessage::DenoEvent(client, generation, event, reply)
            })
            .collect();
        if let Some(cached) = self.cached_events_for_pending_plugins.get_mut(&plugin_id) {
            cached.extend(messages);
        } else {
            self.plugin_executor.execute_for_plugin_tagged(
                plugin_id,
                batch_label,
                batch_has_input,
                move |senders, plugin_map, connected_clients, _, _| {
                    let _shutdown = shutdown_sender;
                    let instances = {
                        let mut map = plugin_map.lock().unwrap();
                        if let Some(workers) = map.deno_worker_routes(plugin_id, generation) {
                            for message in &messages {
                                if let EventOrPipeMessage::DenoEvent(client, _, event, reply) =
                                    message
                                {
                                    workers.send(
                                        generation,
                                        crate::deno::background_worker::Message::Event(
                                            *client,
                                            event.clone(),
                                            *reply,
                                        ),
                                    );
                                }
                            }
                            return;
                        }
                        if !map.contains_deno_generation(plugin_id, generation) {
                            return;
                        }
                        map.running_plugins_and_subscriptions()
                    };
                    let connected_clients = connected_clients.lock().unwrap().clone();
                    for (id, owner, instance, subscriptions) in instances {
                        if id != plugin_id {
                            continue;
                        }
                        let subs = subscriptions.lock().unwrap().clone();
                        let mut plugin = instance.lock().unwrap();
                        let shared = plugin.env().plugin_config.is_shared();
                        let events: Vec<_> = messages
                            .iter()
                            .filter_map(|message| {
                                let client = message.client_for_replay(
                                    owner,
                                    plugin.deno_generation(),
                                    shared,
                                )?;
                                let EventOrPipeMessage::DenoEvent(_, _, event, reply) = message
                                else {
                                    unreachable!();
                                };
                                if let Some(reply) = reply {
                                    reply.claim()?;
                                }
                                let event_type = EventType::from(event);
                                (subs.contains(&event_type)
                                    || event_type == EventType::PermissionRequestResult)
                                    .then(|| (client, event.clone()))
                            })
                            .collect();
                        if events.is_empty() {
                            continue;
                        }
                        let clients = if shared {
                            connected_clients.clone()
                        } else {
                            vec![owner]
                        };
                        let mut assets = vec![];
                        let result = apply_events_to_plugin(
                            plugin_id,
                            &clients,
                            &mut plugin,
                            &events,
                            &mut assets,
                            senders.clone(),
                            &subs,
                        );
                        let disposed = plugin.is_disposed();
                        let instance_id = plugin.instance_id;
                        drop(plugin);
                        match result {
                            Ok(()) => {
                                let _ =
                                    senders.send_to_screen(ScreenInstruction::PluginBytes(assets));
                            },
                            Err(_) if disposed => {},
                            Err(error) => handle_plugin_crash(
                                plugin_id,
                                Some(generation),
                                Some(instance_id),
                                format!("{error:#}"),
                                senders.clone(),
                            ),
                        }
                    }
                },
            );
        }
        Ok(())
    }

    /// A pane-tab key may hand focus to the host. Finish that plugin event before the
    /// client's router sends its next key, so native input cannot overtake queued input.
    pub fn input_barrier(&self, plugin_id: PluginId, completion: yinz_bus::route::NotificationEnd) {
        self.plugin_executor.execute_for_plugin_tagged(
            plugin_id,
            "input_completion",
            true,
            move |_, _, _, _, _| drop(completion),
        );
    }

    pub fn update_plugins(
        &mut self,
        mut updates: Vec<(Option<PluginId>, Option<ClientId>, Event)>,
        shutdown_sender: Sender<()>,
    ) -> Result<()> {
        // The screen owns pane state but has no plugin map. Fill the current session's
        // plugin identities here, before forwarding or caching its manifest.
        for (_, _, event) in &mut updates {
            if let Event::SessionUpdate(sessions, _) = event {
                if let Some(current) = sessions
                    .iter_mut()
                    .find(|session| session.is_current_session)
                {
                    current.populate_plugin_list(self.plugin_map.lock().unwrap().list_plugins());
                }
            }
        }
        self.forward_events_to_cli_subscribers(&updates);
        for (plugin_id, client_id, event) in updates.iter() {
            if plugin_id.is_none() && client_id.is_none() {
                if let Event::HostTerminalThemeChanged(mode) = event {
                    self.last_host_terminal_theme_mode = Some(*mode);
                }
            }
        }
        let plugins_to_update: Vec<RunningPluginEntry> = self
            .plugin_map
            .lock()
            .unwrap()
            .running_plugins_and_subscriptions()
            .iter()
            .filter(
                |&(plugin_id, _client_id, _running_plugin, _subscriptions)| {
                    !&self
                        .cached_events_for_pending_plugins
                        .contains_key(plugin_id)
                },
            )
            .cloned()
            .collect();

        // One job per plugin instance carrying every event of the batch it takes, so a batch of
        // wheel notches costs one render per plugin rather than one per notch.
        let plugin_executor = self.plugin_executor.clone();
        for (plugin_id, client_id, running_plugin, subscriptions) in &plugins_to_update {
            let subs = subscriptions.lock().unwrap().clone();
            let plugin_is_shared = self.plugin_is_shared(plugin_id);
            let events: Vec<(ClientId, Event)> = updates
                .iter()
                .filter_map(|(pid, cid, event)| {
                    let event_type = EventType::from(event);
                    let wanted = (subs.contains(&event_type)
                        || event_type == EventType::PermissionRequestResult)
                        && Self::message_is_directed_at_plugin(
                            *pid,
                            *cid,
                            plugin_id,
                            client_id,
                            plugin_is_shared,
                        );
                    if !wanted {
                        return None;
                    }
                    // for a shared plugin, the acting client is whoever the message was
                    // directed at, not the client the instance is keyed under
                    let acting_client_id = if plugin_is_shared {
                        cid.unwrap_or(*client_id)
                    } else {
                        *client_id
                    };
                    Some((acting_client_id, event.clone()))
                })
                .collect();
            if events.is_empty() {
                continue;
            }
            let render_clients = if plugin_is_shared {
                self.connected_clients.lock().unwrap().clone()
            } else {
                vec![*client_id]
            };
            let (batch_label, batch_has_input) =
                summarize_batch_for_stats(events.iter().map(|(_, event)| event));
            // Execute directly on pinned thread (no async I/O needed for event processing)
            plugin_executor.execute_for_plugin_tagged(*plugin_id, batch_label, batch_has_input, {
                let plugin_id = *plugin_id;
                let running_plugin = running_plugin.clone();
                let _s = shutdown_sender.clone();
                move |senders, _plugin_map, _connected_clients, _plugin_cache, _engine| {
                    let _s = _s; // guard to allow the task to complete before cleanup/shutdown
                    let mut running_plugin = running_plugin.lock().unwrap();
                    let mut plugin_render_assets = vec![];
                    let apply_result = apply_events_to_plugin(
                        plugin_id,
                        &render_clients,
                        &mut running_plugin,
                        &events,
                        &mut plugin_render_assets,
                        senders.clone(),
                        &subs,
                    );
                    // The screen channel is bounded and blocks when the screen
                    // thread is behind; sending while holding the plugin's lock
                    // would pass that stall on to every other job for this
                    // plugin.
                    let disposed = running_plugin.is_disposed();
                    let generation = running_plugin.env().deno_generation;
                    let instance_id = running_plugin.instance_id;
                    drop(running_plugin);
                    match apply_result {
                        Ok(()) => {
                            let _ = senders.send_to_screen(ScreenInstruction::PluginBytes(
                                plugin_render_assets,
                            ));
                        },
                        Err(_) if disposed => {},
                        Err(e) => {
                            log::error!("{:?}", e);

                            // https://stackoverflow.com/questions/66450942/in-rust-is-there-a-way-to-make-literal-newlines-in-r-using-windows-c
                            let stringified_error = format!("{:?}", e).replace("\n", "\n\r");

                            handle_plugin_crash(
                                plugin_id,
                                generation,
                                Some(instance_id),
                                stringified_error,
                                senders.clone(),
                            );
                        },
                    }
                }
            });
        }

        // loop once more to update the cached events for the pending plugins (probably currently
        // being loaded, we'll send them these events when they load)
        for (pid, _cid, event) in updates.drain(..) {
            for (plugin_id, cached_events) in self.cached_events_for_pending_plugins.iter_mut() {
                if pid.is_none() || pid.as_ref() == Some(plugin_id) {
                    cached_events.push(EventOrPipeMessage::Event(event.clone()));
                }
            }
        }
        Ok(())
    }
    pub fn get_plugin_cwd(&self, plugin_id: PluginId, client_id: ClientId) -> Option<PathBuf> {
        self.plugin_map
            .lock()
            .unwrap()
            .instance_cwd(plugin_id, Some(client_id))
    }
    pub fn change_plugin_host_dir(
        &mut self,
        new_host_dir: PathBuf,
        plugin_id: PluginId,
        client_id: ClientId,
        generation: Option<u64>,
    ) -> Result<()> {
        self.plugin_executor.execute_for_plugin_tagged(
            plugin_id,
            "change_host_dir",
            false,
            move |senders, plugin_map, _, _, _| {
                // Resolve on the executor: a queued reload may have replaced the requester.
                let (worker, instances) = {
                    let mut map = plugin_map.lock().unwrap();
                    let worker = generation.is_some_and(|generation| {
                        map.deno_worker_routes(plugin_id, generation).is_some()
                    });
                    if !worker
                        && generation.is_some_and(|generation| {
                            !map.contains_deno_generation(plugin_id, generation)
                        })
                    {
                        return;
                    }
                    (worker, map.running_plugins())
                };
                let reply = |event| {
                    if let Event::FailedToChangeHostFolder(Some(reason)) = &event {
                        Err::<(), _>(anyhow!("{reason}"))
                            .context("failed to change plugin host folder")
                            .non_fatal();
                    }
                    let _ = senders.send_to_plugin(PluginInstruction::for_plugin_event(
                        plugin_id, client_id, generation, event,
                    ));
                };
                match new_host_dir.try_exists() {
                    Ok(false) => {
                        reply(Event::FailedToChangeHostFolder(Some(format!(
                            "Folder {} does not exist",
                            new_host_dir.display()
                        ))));
                        return;
                    },
                    Err(error) => {
                        reply(Event::FailedToChangeHostFolder(Some(error.to_string())));
                        return;
                    },
                    Ok(true) => {},
                }
                if worker {
                    // The worker updates its own environment before delivering this event.
                    reply(Event::HostFolderChanged(new_host_dir));
                    return;
                }
                for (id, owner, instance) in instances {
                    if id != plugin_id {
                        continue;
                    }
                    let mut plugin = instance.lock().unwrap();
                    if generation.map_or(owner != client_id, |generation| {
                        plugin.deno_generation() != Some(generation)
                    }) {
                        continue;
                    }
                    let env = plugin.env_mut();
                    if env.wasi_ctx.is_some() {
                        let wasi_ctx = PluginLoader::create_wasi_ctx(
                            &new_host_dir,
                            &env.plugin_own_data_dir,
                            &env.plugin_own_cache_dir,
                            &YINZ_TMP_DIR,
                            env.user_plugin_dir.as_ref(),
                            &env.plugin_config.location.to_string(),
                            env.plugin_id,
                            env.stdin_pipe.clone(),
                            env.stdout_pipe.clone(),
                        );
                        match wasi_ctx {
                            Ok(wasi_ctx) => drop(env.wasi_ctx.replace(wasi_ctx)),
                            Err(error) => {
                                reply(Event::FailedToChangeHostFolder(Some(error.to_string())));
                                return;
                            },
                        }
                    }
                    env.set_host_folder(new_host_dir.clone());
                    drop(plugin);
                    plugin_map.lock().unwrap().set_instance_cwd(
                        plugin_id,
                        owner,
                        new_host_dir.clone(),
                    );
                    let _ = senders.send_to_pty(yinz_bus::pty::PtyInstruction::ReportPluginCwd(
                        plugin_id,
                        new_host_dir.clone(),
                    ));
                    reply(Event::HostFolderChanged(new_host_dir));
                    return;
                }
            },
        );
        Ok(())
    }
    pub fn pipe_messages(
        &mut self,
        messages: Vec<(Option<PluginId>, Option<ClientId>, PipeMessage)>,
        shutdown_sender: Sender<()>,
        notification_end: Option<NotificationEnd>,
    ) -> Result<()> {
        // URL fan-out is several destinations of the same input, not several requests.
        let mut requests = HashMap::new();
        let mut deliveries = vec![];
        for (plugin, client, message) in messages {
            let request = if let PipeSource::Cli(stream) = &message.source {
                Some(
                    *requests
                        .entry(stream.clone())
                        .or_insert_with(|| self.pending_pipes.begin(stream)),
                )
            } else {
                None
            };
            self.admit_pipe(plugin, client, message, request, &mut deliveries);
        }
        self.schedule_pipes(deliveries, shutdown_sender, notification_end);
        self.finish_pipe_requests();
        Ok(())
    }

    /// Resolve live state for both direct delivery and replay. Metadata only identifies a
    /// destination; it never admits a callback. Register the entire fan-out before scheduling.
    fn admit_pipe(
        &mut self,
        plugin: Option<PluginId>,
        client: Option<ClientId>,
        message: PipeMessage,
        request: Option<PipeRequestId>,
        deliveries: &mut Vec<PipeDelivery>,
    ) -> bool {
        let mut admitted = false;
        let mut available = HashSet::new();
        let instances = self.plugin_map.lock().unwrap().pipe_instances();
        for ((id, owner, instance, _), instance_id) in instances {
            let shared = self.plugin_is_shared(&id);
            if !Self::message_is_directed_at_plugin(plugin, client, &id, &owner, shared) {
                continue;
            }
            available.insert(id);
            if self.cached_events_for_pending_plugins.contains_key(&id) {
                continue;
            }
            admitted = true;
            if let Some(request) = request {
                self.pending_pipes.processing(request, id, instance_id);
            }
            deliveries.push(PipeDelivery {
                plugin: id,
                instance_id,
                instance,
                client: if shared {
                    client.unwrap_or(owner)
                } else {
                    owner
                },
                render_clients: if shared {
                    self.connected_clients.lock().unwrap().clone()
                } else {
                    vec![owner]
                },
                message: message.clone(),
                request,
            });
        }
        for (id, cache) in &mut self.cached_events_for_pending_plugins {
            if plugin.is_some_and(|plugin| plugin != *id) {
                continue;
            }
            if !available.contains(id)
                && (self.latest_plugin_errors.contains_key(id)
                    || (self.plugin_ids_waiting_for_permission_request.contains(id)
                        && !self
                            .loading_plugins
                            .iter()
                            .any(|(loading, _)| loading == id)))
            {
                continue;
            }
            admitted = true;
            let load = self.load_ids.get(id).copied().unwrap_or(0);
            if let Some(request) = request {
                self.pending_pipes.cache(request, *id, load);
            }
            cache.push(EventOrPipeMessage::PipeMessage(
                client,
                message.clone(),
                request,
                load,
            ));
        }
        if let (false, Some(plugin), Some(request)) = (admitted, plugin, request) {
            self.pending_pipes.fail(request, format!("Plugin {plugin} is unavailable; explicitly reload it before sending another pipe"));
        }
        admitted
    }

    fn schedule_pipes(
        &mut self,
        deliveries: Vec<PipeDelivery>,
        shutdown: Sender<()>,
        mut notification_end: Option<NotificationEnd>,
    ) {
        for delivery in deliveries {
            let PipeDelivery {
                plugin,
                instance_id,
                instance,
                client,
                render_clients,
                message,
                request,
            } = delivery;
            let guard = shutdown.clone();
            let notification = notification_end.take();
            let accepted = self.plugin_executor.execute_for_plugin_tagged(
                plugin,
                format!("pipe:{}", message.name),
                false,
                move |senders, _, _, _, _| {
                    let _guard = guard;
                    let _notification = notification;
                    let mut instance = instance.lock().unwrap();
                    let mut assets = vec![];
                    let unavailable = instance.is_unavailable();
                    let result = apply_pipe_message_to_plugin(
                        plugin,
                        client,
                        &render_clients,
                        &mut instance,
                        &message,
                        request,
                        &mut assets,
                        &senders,
                    );
                    let generation = instance.env().deno_generation;
                    drop(instance);
                    match result {
                        Ok(()) if !assets.is_empty() => {
                            senders
                                .send_to_screen(ScreenInstruction::PluginBytes(assets))
                                .context("failed to send plugin frame")
                                .non_fatal();
                        },
                        Ok(()) => {},
                        Err(_) if unavailable => {},
                        Err(error) => handle_plugin_crash(
                            plugin,
                            generation,
                            Some(instance_id),
                            format!("{error:#}"),
                            senders,
                        ),
                    }
                },
            );
            if !accepted {
                self.pending_pipes.complete(PipeCompletion {
                    plugin_id: plugin,
                    instance_id,
                    changes: HashMap::new(),
                    callback: request.map(|request| {
                        (
                            request,
                            Err(format!("Plugin {plugin} executor rejected pipe delivery")),
                        )
                    }),
                });
            }
        }
    }

    pub fn apply_cached_events_for_load(
        &mut self,
        load_id: u64,
        mut plugin_ids: Vec<PluginId>,
        done_receiving_permissions: bool,
        shutdown_sender: Sender<()>,
    ) -> Result<()> {
        plugin_ids.retain(|id| self.load_is_current(*id, load_id));
        self.apply_cached_events(plugin_ids, done_receiving_permissions, shutdown_sender)
    }

    pub fn apply_cached_events(
        &mut self,
        plugin_ids: Vec<PluginId>,
        done_receiving_permissions: bool,
        shutdown_sender: Sender<()>,
    ) -> Result<()> {
        let mut applied_plugin_paths = HashSet::new();
        for plugin_id in plugin_ids.iter().copied() {
            // The loader has returned, even if a live instance still needs permission.
            // Keeping it marked as loading would admit pipes after that instance fails.
            if let Some(run_plugin) = self.run_plugin_of_loading_plugin_id(plugin_id) {
                applied_plugin_paths.insert(run_plugin.clone());
                self.refresh_loaded_plugin_location(plugin_id);
            }
            self.loading_plugins
                .retain(|(p_id, _run_plugin)| p_id != &plugin_id);
            if self
                .permission_requests
                .values()
                .any(|pending| pending.contains_plugin(plugin_id))
                || (!done_receiving_permissions
                    && self
                        .plugin_ids_waiting_for_permission_request
                        .contains(&plugin_id)
                    && self
                        .plugin_map
                        .lock()
                        .unwrap()
                        .pipe_instances()
                        .iter()
                        .any(|((id, _, _, _), _)| *id == plugin_id))
            {
                continue;
            }
            self.plugin_ids_waiting_for_permission_request
                .remove(&plugin_id);
            self.apply_cached_events_and_resizes_for_plugin(plugin_id, shutdown_sender.clone())?;
        }
        for plugin_id in plugin_ids {
            if self.pending_plugin_id_reloads.remove(&plugin_id) {
                self.reload_plugin_with_id(plugin_id)?;
            }
        }
        for run_plugin in applied_plugin_paths.drain() {
            if self.pending_plugin_reloads.remove(&run_plugin) {
                let _ = self.reload_plugin(&run_plugin);
            }
        }
        Ok(())
    }
    /// A shared plugin's single instance is keyed in the PluginMap under the client that loaded
    /// it. If that client is gone, hand the instance over to `new_owner` so that it keeps
    /// receiving events.
    fn rekey_shared_plugin_if_owner_gone(&mut self, plugin_id: PluginId, new_owner: ClientId) {
        self.plugin_executor.execute_for_plugin_tagged(
            plugin_id,
            "client_rekey",
            false,
            move |_, plugin_map, connected_clients, _, _| {
                let mut plugin_map = plugin_map.lock().unwrap();
                let connected_clients = connected_clients.lock().unwrap();
                for owner in plugin_map.owner_client_ids_of_plugin_id(plugin_id) {
                    if !connected_clients.contains(&owner) {
                        plugin_map.rekey_client(plugin_id, owner, new_owner);
                    }
                }
            },
        );
    }
    pub fn deno_dispose(&self, plugin_id: PluginId, generation: u64) {
        self.plugin_executor.execute_for_plugin_tagged(
            plugin_id,
            "deno_dispose",
            false,
            move |_, _, _, _, _| super::deno::dispose_generation(generation),
        );
    }

    pub fn deno_palette(
        &self,
        delivery: yinz_bus::command_palette::Delivery,
        shutdown: Sender<()>,
    ) {
        let plugin_id = delivery.owner.plugin_id;
        self.plugin_executor.execute_for_plugin_tagged(
            plugin_id,
            "deno_palette",
            false,
            move |senders, plugin_map, connected_clients, _, _| {
                let _shutdown = shutdown;
                let instances = plugin_map.lock().unwrap().running_plugins();
                for (id, owner, instance) in instances {
                    if id != plugin_id {
                        continue;
                    }
                    let mut plugin = instance.lock().unwrap();
                    if plugin.is_unavailable()
                        || plugin.deno_generation() != Some(delivery.owner.generation)
                        || delivery.active.as_ref().is_some_and(|active| {
                            !active.load(std::sync::atomic::Ordering::Acquire)
                        })
                        || (!plugin.env().plugin_config.is_shared() && owner != delivery.client)
                    {
                        continue;
                    }
                    let is_request = matches!(
                        delivery.message,
                        yinz_types::command_palette::Message::Discover { .. }
                            | yinz_types::command_palette::Message::Execute { .. }
                    );
                    if is_request
                        && (!connected_clients.lock().unwrap().contains(&delivery.client)
                            || matches!(
                                crate::yinz_exports::check_command_permission(
                                    plugin.env(),
                                    &yinz_types::data::PluginCommand::CommandPalette(
                                        yinz_types::command_palette::Command::Unregister {
                                            registration: String::new()
                                        }
                                    )
                                ),
                                (yinz_types::data::PermissionStatus::Denied, _)
                            ))
                    {
                        continue;
                    }
                    let previous =
                        std::mem::replace(&mut plugin.env_mut().client_id, delivery.client);
                    let result = plugin.deno_palette(&delivery.message);
                    plugin.env_mut().client_id = previous;
                    let clients = if plugin.env().plugin_config.is_shared() {
                        connected_clients.lock().unwrap().clone()
                    } else {
                        vec![owner]
                    };
                    let result = result.and_then(|render| {
                        if render && plugin.has_area() {
                            render_plugin_for_clients(plugin_id, &mut plugin, &clients)
                        } else {
                            Ok(vec![])
                        }
                    });
                    match result {
                        Ok(assets) if !assets.is_empty() => senders
                            .send_to_screen(ScreenInstruction::PluginBytes(assets))
                            .context("rendering palette plugin")
                            .non_fatal(),
                        Ok(_) => {},
                        Err(error) => {
                            plugin.mark_failed();
                            handle_plugin_crash(
                                plugin_id,
                                Some(delivery.owner.generation),
                                Some(plugin.instance_id),
                                format!("{error:#}"),
                                senders.clone(),
                            );
                        },
                    }
                    break;
                }
            },
        );
    }

    pub fn deno_tick(&self, plugin_id: PluginId, generation: u64, shutdown: Sender<()>) {
        self.plugin_executor.execute_for_plugin_tagged(
            plugin_id,
            "deno_tick",
            false,
            move |senders, plugin_map, connected_clients, _, _| {
                let _shutdown = shutdown;
                if let Some(workers) = plugin_map
                    .lock()
                    .unwrap()
                    .deno_worker_routes(plugin_id, generation)
                {
                    workers.send(generation, crate::deno::background_worker::Message::Tick);
                    return;
                }
                let connected_clients = connected_clients.lock().unwrap().clone();
                let instances = plugin_map.lock().unwrap().running_plugins();
                for (id, client_id, instance) in instances {
                    if id != plugin_id {
                        continue;
                    }
                    let mut plugin = instance.lock().unwrap();
                    if plugin.is_unavailable() || plugin.deno_generation() != Some(generation) {
                        continue;
                    }
                    let clients = if plugin.env().plugin_config.is_shared() {
                        connected_clients.clone()
                    } else {
                        vec![client_id]
                    };
                    let result = plugin.deno_tick().and_then(|render| {
                        if render && plugin.has_area() {
                            render_plugin_for_clients(plugin_id, &mut plugin, &clients)
                        } else {
                            Ok(vec![])
                        }
                    });
                    if result.is_ok() {
                        report_pipe_changes(&mut plugin, HashMap::new(), None, &senders);
                    }
                    if result.is_err() {
                        plugin.mark_failed();
                    }
                    let instance_id = plugin.instance_id;
                    let disposed = plugin.is_disposed();
                    drop(plugin);
                    match result {
                        Ok(assets) if !assets.is_empty() => {
                            senders
                                .send_to_screen(ScreenInstruction::PluginBytes(assets))
                                .context("failed to deliver asynchronous plugin render")
                                .non_fatal();
                        },
                        Ok(_) => {},
                        Err(_) if disposed => {},
                        Err(error) => handle_plugin_crash(
                            plugin_id,
                            Some(generation),
                            Some(instance_id),
                            format!("{error:#}"),
                            senders.clone(),
                        ),
                    }
                    break;
                }
            },
        );
    }

    pub fn remove_client(&mut self, client_id: ClientId) {
        self.cancel_client_picks(client_id);
        self.client_cancellations
            .lock()
            .unwrap()
            .entry(client_id)
            .or_default()
            .store(true, Ordering::Release);
        let was_connected = {
            let mut clients = self.connected_clients.lock().unwrap();
            let previous_len = clients.len();
            clients.retain(|client| *client != client_id);
            clients.len() != previous_len
        };
        if self.cli_event_subscribers.remove(&client_id).is_some()
            && self.cli_event_subscribers.is_empty()
        {
            let _ = self
                .senders
                .send_to_screen(ScreenInstruction::SetCliEventSubscribersPresent(false));
        }
        if let Some(new_owner) = self.get_first_client_id() {
            for plugin_id in self.shared_plugin_ids.clone() {
                self.rekey_shared_plugin_if_owner_gone(plugin_id, new_owner);
            }
        }

        // CLI connections never owned instances. Do not enqueue work for every plugin
        // on each CLI disconnect (agent hooks and session probes are frequent).
        if was_connected {
            let plugin_ids: Vec<_> = self.cancel_tokens.keys().copied().collect();
            for plugin_id in plugin_ids {
                if self.shared_plugin_ids.contains(&plugin_id) {
                    continue;
                }
                self.plugin_executor.execute_for_plugin_tagged(
                    plugin_id,
                    "remove client",
                    false,
                    move |senders, plugin_map, connected_clients, _, _| {
                        cleanup_disconnected_instances(
                            plugin_id,
                            &connected_clients,
                            &plugin_map,
                            senders,
                        );
                    },
                );
            }
        }

        // Remove client from cached pane render report
        if let Some(ref mut prev_report) = self.previous_pane_render_report {
            prev_report.all_pane_contents.remove(&client_id);
        }
    }

    fn get_changed_panes_per_client(
        &self,
        new_contents: &HashMap<
            yinz_types::data::ClientId,
            HashMap<yinz_types::data::PaneId, PaneContents>,
        >,
        previous_contents: Option<
            &HashMap<yinz_types::data::ClientId, HashMap<yinz_types::data::PaneId, PaneContents>>,
        >,
    ) -> HashMap<yinz_types::data::ClientId, HashMap<yinz_types::data::PaneId, PaneContents>> {
        let mut result: HashMap<
            yinz_types::data::ClientId,
            HashMap<yinz_types::data::PaneId, PaneContents>,
        > = HashMap::new();

        // First report - return everything grouped by client
        let Some(prev_contents) = previous_contents else {
            for (client_id, panes) in new_contents {
                result.insert(*client_id, panes.clone());
            }
            return result;
        };

        // Compare each client's panes
        for (client_id, new_panes) in new_contents {
            let mut client_panes: HashMap<yinz_types::data::PaneId, PaneContents> = HashMap::new();
            for (pane_id, new_pane_contents) in new_panes {
                let has_changed = prev_contents
                    .get(client_id)
                    .and_then(|prev_panes| prev_panes.get(pane_id))
                    .map(|prev_pane_contents| {
                        prev_pane_contents.viewport != new_pane_contents.viewport
                            || prev_pane_contents.selected_text != new_pane_contents.selected_text
                            || prev_pane_contents.cursor != new_pane_contents.cursor
                    })
                    .unwrap_or(true);
                if has_changed {
                    client_panes.insert(*pane_id, new_pane_contents.clone());
                }
            }
            if !client_panes.is_empty() {
                result.insert(*client_id, client_panes);
            }
        }
        result
    }

    pub fn handle_pane_render_report(
        &mut self,
        pane_render_report: PaneRenderReport,
        shutdown_sender: Sender<()>,
    ) -> Result<()> {
        let changed_panes_per_client = self.get_changed_panes_per_client(
            &pane_render_report.all_pane_contents,
            self.previous_pane_render_report
                .as_ref()
                .map(|r| &r.all_pane_contents),
        );
        for (client_id, client_panes) in changed_panes_per_client {
            let updates = vec![(None, Some(client_id), Event::PaneRenderReport(client_panes))];
            self.update_plugins(updates, shutdown_sender.clone())?;
        }

        // Deferred clients are absent from this report. Keep their comparison history until
        // they render again or disconnect, replacing the full pane map for clients that rendered.
        self.previous_pane_render_report
            .get_or_insert_with(PaneRenderReport::default)
            .all_pane_contents
            .extend(pane_render_report.all_pane_contents);
        Ok(())
    }

    /// Replay the last broadcast `HostTerminalThemeChanged` to a plugin that
    /// subscribed to it after the broadcast. Nothing is sent while the hue is
    /// still unknown.
    pub fn send_host_terminal_theme_mode_to_plugin(
        &self,
        plugin_id: PluginId,
        client_id: ClientId,
    ) {
        let Some(mode) = self.last_host_terminal_theme_mode else {
            return;
        };
        self.senders
            .send_to_plugin(PluginInstruction::Update(vec![(
                Some(plugin_id),
                Some(client_id),
                Event::HostTerminalThemeChanged(mode),
            )]))
            .context("failed to replay the host terminal theme mode to a late subscriber")
            .non_fatal();
    }

    pub fn send_initial_keybinds_to_plugin(&self, plugin_id: PluginId, client_id: ClientId) {
        // Use the same authoritative input as LoadingContext and reconfigure.
        // Reading a guest's copy here can deadlock its synchronous host request.
        let keybinds = self
            .keybinds
            .get(&client_id)
            .unwrap_or(&self.default_keybinds);
        self.send_keybinds_payload_to_plugin(plugin_id, client_id, keybinds.to_keybinds_vec());
    }

    fn send_keybinds_payload_to_plugin(
        &self,
        plugin_id: PluginId,
        client_id: ClientId,
        keybinds: KeybindsVec,
    ) {
        let _ = self.senders.send_to_plugin(PluginInstruction::Update(vec![(
            Some(plugin_id),
            Some(client_id),
            Event::InitialKeybinds(keybinds),
        )]));
    }

    /// Stop every guest: an instance looping in `update()` or `load()` releases its lock and
    /// its executor thread at its next check of the token. Shutdown does this once in-flight
    /// jobs have had their grace and not finished, so that the unloads in `cleanup` find the
    /// guests already stopped rather than each waiting on them in turn.
    pub fn cancel_all_plugins(&self) {
        for cancel in self.cancel_tokens.values() {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Unload everything at shutdown, giving up on any lock still held at `deadline`. One
    /// absolute deadline for the whole of it, shared with the wait that precedes it: the
    /// plugin thread has `PLUGIN_THREAD_EXIT_TIMEOUT` in all before its join is abandoned, and
    /// a budget that re-armed per lock added up to more than that.
    pub fn cleanup(&mut self, deadline: Instant) {
        // Before any unload: the unload hook pumps queues, and nothing may start now.
        self.queues.lock().unwrap().shut_down();

        // Already done by the shutdown sequence if a job overran its grace; for every other
        // guest this is the first it hears of it, and for a caller that skipped the wait
        // (a test) it is the only cancel there is.
        self.cancel_all_plugins();

        // A plugin whose loader has not run yet is not in the plugin map, so the loop below
        // would miss it and its loader would instantiate it after this thread is gone. Unload
        // it first: that is what records the cancellation the loader checks for.
        let still_loading: Vec<PluginId> =
            self.loading_plugins.iter().map(|(pid, _)| *pid).collect();
        for plugin_id in still_loading {
            drop(self.unload_plugin_with_deadline(plugin_id, Some(deadline)));
        }

        let plugin_ids = match lock_with_deadline(&self.plugin_map, Some(deadline)) {
            Some(plugin_map) => plugin_map.plugin_ids(),
            None => {
                log::error!(
                    "plugins could not be unloaded at shutdown: a plugin's loader is still \
                     running and did not stop"
                );
                vec![]
            },
        };
        for plugin_id in &plugin_ids {
            drop(self.unload_plugin_with_deadline(*plugin_id, Some(deadline)));
        }
    }
    /// Drop the host and let all loaders, environments and workers release /data within
    /// the original shutdown deadline. A runaway is recovered on the next startup.
    pub fn wait_for_cleanup(mut self, deadline: Instant) {
        let done = self.temporary_cleanup_done.take();
        drop(self);
        if let Some(done) = done {
            if let Err(std::sync::mpsc::RecvTimeoutError::Timeout) =
                done.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                log::warn!("plugin temporary data still in use at the shutdown deadline");
            }
        }
    }

    pub fn run_plugin_of_loading_plugin_id(&self, plugin_id: PluginId) -> Option<&RunPlugin> {
        self.loading_plugins
            .iter()
            .find(|(p_id, _run_plugin)| p_id == &plugin_id)
            .map(|(_p_id, run_plugin)| run_plugin)
    }
    pub fn run_plugin_of_plugin_id(&self, plugin_id: PluginId) -> Option<RunPlugin> {
        self.plugin_map
            .lock()
            .unwrap()
            .run_plugin_of_plugin_id(plugin_id)
    }

    pub fn reconfigure(
        &mut self,
        client_id: ClientId,
        keybinds: Option<Keybinds>,
        default_mode: Option<InputMode>,
        default_shell: Option<TerminalAction>,
        layout_dir: Option<PathBuf>,
        user_plugin_dir: Option<PathBuf>,
        builtin_plugins_dir: Option<PathBuf>,
    ) -> Result<()> {
        let plugins_to_reconfigure: Vec<(PluginId, Arc<Mutex<RunningPlugin>>)> = self
            .plugin_map
            .lock()
            .unwrap()
            .running_plugins()
            .iter()
            .cloned()
            .filter_map(|(plugin_id, c_id, running_plugin)| {
                if c_id == client_id {
                    Some((plugin_id, running_plugin.clone()))
                } else {
                    None
                }
            })
            .collect();
        if let Some(default_mode) = default_mode.as_ref() {
            self.base_modes.insert(client_id, *default_mode);
        }
        if let Some(keybinds) = keybinds.as_ref() {
            self.keybinds.insert(client_id, keybinds.clone());
        }
        self.default_shell = default_shell.clone();
        self.layout_dir = layout_dir.clone();
        self.user_plugin_dir = user_plugin_dir.clone();
        self.builtin_plugins_dir = builtin_plugins_dir;
        // Collect plugins subscribed to InitialKeybinds for post-reconfigure notification
        let plugins_subscribed_to_initial_keybinds: Vec<PluginId> = if keybinds.is_some() {
            self.plugin_map
                .lock()
                .unwrap()
                .running_plugins_and_subscriptions()
                .iter()
                .filter(|(_, cid, _, subs)| {
                    *cid == client_id && subs.lock().unwrap().contains(&EventType::InitialKeybinds)
                })
                .map(|(pid, _, _, _)| *pid)
                .collect()
        } else {
            vec![]
        };

        for (plugin_id, running_plugin) in plugins_to_reconfigure {
            self.plugin_executor
                .execute_for_plugin_tagged(plugin_id, "reconfigure", false, {
                    let running_plugin = running_plugin.clone();
                    let keybinds = keybinds.clone();
                    let default_shell = default_shell.clone();
                    let layout_dir = layout_dir.clone();
                    let user_plugin_dir = user_plugin_dir.clone();
                    move |_senders, _plugin_map, _connected_clients, _plugin_cache, _engine| {
                        let mut running_plugin = running_plugin.lock().unwrap();
                        if let Some(keybinds) = keybinds {
                            running_plugin.update_keybinds(keybinds);
                        }
                        if let Some(default_mode) = default_mode {
                            running_plugin.update_default_mode(default_mode);
                        }
                        running_plugin.update_default_shell(default_shell);
                        running_plugin.update_layout_dir(layout_dir);
                        running_plugin.update_user_plugin_dir(user_plugin_dir);
                    }
                });
        }
        // Send InitialKeybinds to subscribed plugins after reconfiguration. The payload is built
        // from the reconfigure request rather than read back from each plugin's env: the env is
        // updated on the executor above, asynchronously, so reading it here could still see the
        // old keybinds.
        if let Some(keybinds) = keybinds.as_ref() {
            let keybinds_payload = keybinds.to_keybinds_vec();
            for plugin_id in plugins_subscribed_to_initial_keybinds {
                self.send_keybinds_payload_to_plugin(
                    plugin_id,
                    client_id,
                    keybinds_payload.clone(),
                );
            }
        }
        Ok(())
    }
    fn apply_cached_events_and_resizes_for_plugin(
        &mut self,
        plugin_id: PluginId,
        shutdown_sender: Sender<()>,
    ) -> Result<()> {
        if let Some(messages) = self.cached_events_for_pending_plugins.remove(&plugin_id) {
            let mut events = vec![];
            for message in messages {
                if let EventOrPipeMessage::PipeMessage(client, message, request, load) = message {
                    self.replay_events(
                        plugin_id,
                        std::mem::take(&mut events),
                        shutdown_sender.clone(),
                    );
                    let mut deliveries = vec![];
                    let admitted =
                        self.admit_pipe(Some(plugin_id), client, message, request, &mut deliveries);
                    if let Some(request) = request {
                        self.pending_pipes.finish_cached(
                            request,
                            plugin_id,
                            load,
                            if admitted {
                                Ok(())
                            } else {
                                Err(format!(
                                    "Plugin {plugin_id} has no recipient for cached pipe delivery"
                                ))
                            },
                        );
                    }
                    self.schedule_pipes(deliveries, shutdown_sender.clone(), None);
                } else {
                    events.push(message);
                }
            }
            self.replay_events(plugin_id, events, shutdown_sender.clone());
            self.finish_pipe_requests();
        }
        if let Some((rows, columns)) = self.cached_resizes_for_pending_plugins.remove(&plugin_id) {
            self.resize_plugin(plugin_id, columns, rows, shutdown_sender.clone())?;
        }
        self.apply_cached_worker_messages(plugin_id)?;
        Ok(())
    }
    // Cached events retain their subscription and acting-client semantics. Pipe messages
    // between these batches use the same admission path as live delivery.
    fn replay_events(
        &self,
        plugin_id: PluginId,
        events: Vec<EventOrPipeMessage>,
        shutdown: Sender<()>,
    ) {
        if events.is_empty() {
            return;
        }
        let instances = self
            .plugin_map
            .lock()
            .unwrap()
            .running_plugins_and_subscriptions();
        let shared = self.plugin_is_shared(&plugin_id);
        for (id, owner, instance, subscriptions) in instances {
            if id != plugin_id {
                continue;
            }
            let events = events.clone();
            let subs = subscriptions.lock().unwrap().clone();
            let clients = if shared {
                self.connected_clients.lock().unwrap().clone()
            } else {
                vec![owner]
            };
            let guard = shutdown.clone();
            self.plugin_executor.execute_for_plugin_tagged(
                plugin_id,
                "cached_events",
                false,
                move |senders, plugin_map, _, _, _| {
                    let _guard = guard;
                    // A cached notification still belongs to its original instance after
                    // unload removes that instance from the map. Leave it for cleanup.
                    if !plugin_map
                        .lock()
                        .unwrap()
                        .running_plugins_and_subscriptions()
                        .iter()
                        .any(|(_, _, current, _)| Arc::ptr_eq(current, &instance))
                    {
                        return;
                    }
                    for message in events {
                        let mut plugin = instance.lock().unwrap();
                        if plugin.is_unavailable() {
                            break;
                        }
                        if let EventOrPipeMessage::DenoEvent(client, generation, event, reply) =
                            &message
                        {
                            if let Some(workers) = &plugin.env().deno_workers {
                                if workers.contains(*generation) {
                                    workers.send(
                                        *generation,
                                        crate::deno::background_worker::Message::Event(
                                            *client,
                                            event.clone(),
                                            *reply,
                                        ),
                                    );
                                    continue;
                                }
                            }
                        }
                        let Some(client) = message.client_for_replay(
                            plugin.env().owner_client_id,
                            plugin.deno_generation(),
                            shared,
                        ) else {
                            continue;
                        };
                        let event = match message {
                            EventOrPipeMessage::Event(event) => event,
                            EventOrPipeMessage::DenoEvent(_, _, event, reply) => {
                                if reply.is_some_and(|reply| reply.claim().is_none()) {
                                    continue;
                                }
                                event
                            },
                            EventOrPipeMessage::PipeMessage(..) => continue,
                        };
                        if !subs.contains(&EventType::from(&event))
                            && !(matches!(event, Event::PermissionRequestResult(_))
                                && plugin.deno_generation().is_some())
                        {
                            continue;
                        }
                        let mut assets = vec![];
                        let result = apply_events_to_plugin(
                            plugin_id,
                            &clients,
                            &mut plugin,
                            &[(client, event)],
                            &mut assets,
                            senders.clone(),
                            &subs,
                        );
                        let generation = plugin.env().deno_generation;
                        let instance_id = plugin.instance_id;
                        drop(plugin);
                        match result {
                            Ok(()) if !assets.is_empty() => {
                                senders
                                    .send_to_screen(ScreenInstruction::PluginBytes(assets))
                                    .context("failed to render cached plugin events")
                                    .non_fatal();
                            },
                            Ok(()) => {},
                            Err(error) => handle_plugin_crash(
                                plugin_id,
                                generation,
                                Some(instance_id),
                                format!("{error:#}"),
                                senders.clone(),
                            ),
                        }
                    }
                },
            );
        }
    }

    pub fn apply_cached_worker_messages(&mut self, plugin_id: PluginId) -> Result<()> {
        if let Some(mut messages) = self.cached_worker_messages.remove(&plugin_id) {
            let mut worker_messages: HashMap<(ClientId, String), Vec<(String, String)>> =
                HashMap::new();
            for (client_id, worker_name, message, payload) in messages.drain(..) {
                worker_messages
                    .entry((client_id, worker_name))
                    .or_default()
                    .push((message, payload));
            }
            for ((client_id, worker_name), messages) in worker_messages.drain() {
                self.post_messages_to_plugin_worker(plugin_id, client_id, worker_name, messages)?;
            }
        }
        Ok(())
    }
    fn plugin_id_of_loading_plugin(
        &self,
        plugin_location: &RunPluginLocation,
        plugin_configuration: &PluginUserConfiguration,
    ) -> Option<PluginId> {
        let location = self
            .normalize_plugin(RunPlugin {
                location: plugin_location.clone(),
                ..Default::default()
            })
            .location;
        let wanted = plugin_configuration.without_caller_context();
        self.loading_plugins
            .iter()
            .find_map(|(plugin_id, run_plugin)| {
                if self.normalize_plugin(run_plugin.clone()).location == location
                    && run_plugin.configuration.without_caller_context() == wanted
                {
                    Some(*plugin_id)
                } else {
                    None
                }
            })
    }
    /// Whether an instance of `run_plugin` is already running or on its way up.
    ///
    /// Retry metadata also counts here: a failed hook keeps its pane until explicit reload.
    pub(crate) fn plugin_instance_exists(&self, run_plugin: &RunPlugin) -> bool {
        if self
            .plugin_id_of_loading_plugin(&run_plugin.location, &run_plugin.configuration)
            .is_some()
        {
            return true;
        }
        self.all_plugin_ids_for_plugin_location(&run_plugin.location, &run_plugin.configuration)
            .map(|plugin_ids| !plugin_ids.is_empty())
            .unwrap_or(false)
    }
    #[cfg(test)]
    pub(crate) fn get_plugin_ids_for_url(&self, url: &str) -> Result<Vec<PluginId>, String> {
        self.get_plugin_ids_for_url_with_configuration(url, None)
    }

    pub(crate) fn get_plugin_ids_for_url_with_configuration(
        &self,
        url: &str,
        configuration: Option<BTreeMap<String, String>>,
    ) -> Result<Vec<PluginId>, String> {
        let aliases = self.plugin_aliases.lock().map_err(|e| e.to_string())?;
        let plugin = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
            url,
            &configuration,
            Some(&aliases),
            None,
        )?
        .get_run_plugin()
        .ok_or_else(|| format!("{url} is not a plugin or a known alias"))?;
        drop(aliases);
        match self.all_plugin_ids_for_plugin_location(&plugin.location, &plugin.configuration) {
            Ok(ids) => Ok(ids),
            Err(error)
                if matches!(
                    error.downcast_ref::<YinzError>(),
                    Some(YinzError::PluginDoesNotExist)
                ) =>
            {
                Ok(vec![])
            },
            Err(error) => Err(format!("{error:#}")),
        }
    }

    fn all_plugin_ids_for_plugin_location(
        &self,
        plugin_location: &RunPluginLocation,
        plugin_configuration: &PluginUserConfiguration,
    ) -> Result<Vec<PluginId>> {
        self.plugin_map
            .lock()
            .unwrap()
            .all_plugin_ids_for_plugin_location(plugin_location, plugin_configuration, |location| {
                self.normalize_plugin(RunPlugin {
                    location: location.clone(),
                    ..Default::default()
                })
                .location
            })
    }
    pub(crate) fn palette_note(&self, request: yinz_bus::command_palette::NoteRequest) {
        let data = self.data_access.clone();
        let senders = self.senders.clone();
        let generations = self.deno_watch_generations.clone();
        yinz_bus::global_async_runtime::get_tokio_runtime().spawn_blocking(move || {
            let editing = matches!(
                request.action,
                yinz_bus::command_palette::NoteAction::Open(_)
            );
            crate::yinz_exports::palette_notes::handle(request, &senders, &data);
            if editing {
                crate::yinz_exports::notify_data_watchers(
                    data.plugin_data(),
                    &generations,
                    &senders,
                    project_data::store::SHARED,
                    &std::collections::BTreeSet::from(["note".into()]),
                );
            }
        });
    }

    pub(crate) fn cli_data(&self, request: yinz_types::cli::DataRequest) -> anyhow::Result<String> {
        crate::cli_data::run(
            self.data_access.plugin_data(),
            request,
            |namespace, entities| {
                crate::yinz_exports::notify_data_watchers(
                    self.data_access.plugin_data(),
                    &self.deno_watch_generations,
                    &self.senders,
                    namespace,
                    entities,
                );
            },
        )
    }

    pub fn builtin_service_instance(&self, name: &str) -> Result<(PluginId, ClientId), String> {
        let instances: std::collections::BTreeMap<_, _> = self
            .plugin_map
            .lock()
            .unwrap()
            .clone_plugin_assets()
            .get(&RunPluginLocation::Yinz(yinz_types::data::PluginTag::new(
                name,
            )))
            .into_iter()
            .flat_map(|configurations| configurations.values())
            .flatten()
            .copied()
            .collect();
        match instances.len() {
            0 => Err(format!("No running {name} instance; load it as a background plugin before requesting this operation")),
            1 => instances.into_iter().next().ok_or_else(|| "service disappeared".into()),
            _ => Err(format!("Multiple {name} instances: {:?}; keep one background instance before requesting this operation", instances.keys().collect::<Vec<_>>())),
        }
    }

    pub fn all_plugin_and_client_ids_for_plugin_location(
        &mut self,
        plugin_location: &RunPluginLocation,
        plugin_configuration: &PluginUserConfiguration,
    ) -> Vec<(PluginId, Option<ClientId>)> {
        let normalized = self
            .normalize_plugin(RunPlugin {
                location: plugin_location.clone(),
                ..Default::default()
            })
            .location;
        let plugin_location = &normalized;
        let identities = self.plugin_map.lock().unwrap().clone_plugin_assets();
        let mut destinations: Vec<_> = identities
            .iter()
            .filter(|(location, _)| {
                self.normalize_plugin(RunPlugin {
                    location: (*location).clone(),
                    ..Default::default()
                })
                .location
                    == *plugin_location
            })
            .flat_map(|(_, configurations)| instances_of(configurations, plugin_configuration))
            .map(|(plugin, client)| (*plugin, Some(*client)))
            .collect();
        // A restored layout can finish loading its bridge after the first CLI
        // request loaded one. This service has one recipient, unlike view plugins.
        if crate::host_bridge::is_workflow_bridge(plugin_location) {
            destinations.sort_unstable();
            destinations.truncate(1);
        }
        destinations
    }
    /// The plugins at `plugin_location` with `plugin_configuration` whose load is still in
    /// flight, so not yet in the plugin map. An event addressed to one of these is held in its
    /// cache and applied when the load completes - which makes a loading plugin an instance to
    /// address, not a reason to load another.
    pub fn loading_plugin_ids_for_plugin_location(
        &self,
        plugin_location: &RunPluginLocation,
        plugin_configuration: &PluginUserConfiguration,
    ) -> Vec<PluginId> {
        let normalized = self
            .normalize_plugin(RunPlugin {
                location: plugin_location.clone(),
                ..Default::default()
            })
            .location;
        let plugin_location = &normalized;
        let wanted = plugin_configuration.without_caller_context();
        self.loading_plugins
            .iter()
            .filter(|(_, run_plugin)| {
                &self.normalize_plugin(run_plugin.clone()).location == plugin_location
                    && run_plugin.configuration.without_caller_context() == wanted
            })
            .map(|(plugin_id, _)| *plugin_id)
            .collect()
    }
    #[cfg(test)]
    pub fn all_plugin_ids(&self) -> Vec<(PluginId, ClientId)> {
        self.plugin_map.lock().unwrap().all_plugin_ids()
    }
    /// Whether this plugin runs as one instance for the whole session rather than one per client.
    /// See `PluginConfig::is_shared`; it decides both who a message reaches and who it renders to.
    pub fn plugin_is_shared(&self, plugin_id: &PluginId) -> bool {
        self.shared_plugin_ids.contains(plugin_id)
    }
    fn size_of_plugin_id(&self, plugin_id: PluginId) -> Option<(usize, usize)> {
        self.plugin_map
            .lock()
            .unwrap()
            .reload_metadata(plugin_id)
            .map(|metadata| metadata.size)
    }
    fn cwd_of_plugin_id(&self, plugin_id: PluginId) -> Option<PathBuf> {
        self.plugin_map
            .lock()
            .unwrap()
            .reload_metadata(plugin_id)
            .map(|metadata| metadata.cwd)
    }
    fn plugin_config_of_plugin_id(&self, plugin_id: PluginId) -> Option<PluginConfig> {
        self.plugin_map
            .lock()
            .unwrap()
            .plugin_config_of_plugin_id(plugin_id)
    }
    fn tab_index_of_plugin_id(&self, plugin_id: PluginId) -> Option<TabId> {
        self.plugin_map
            .lock()
            .unwrap()
            .reload_metadata(plugin_id)
            .and_then(|metadata| metadata.tab_index)
    }
    fn start_plugin_loading_indication(
        &self,
        plugin_ids: &[PluginId],
        loading_indication: &LoadingIndication,
    ) {
        for plugin_id in plugin_ids {
            let _ = self
                .senders
                .send_to_screen(ScreenInstruction::StartPluginLoadingIndication(
                    *plugin_id,
                    loading_indication.clone(),
                ));
            let _ = self
                .senders
                .send_to_background_jobs(BackgroundJob::AnimatePluginLoading(*plugin_id));
        }
    }
    pub fn post_messages_to_plugin_worker(
        &mut self,
        plugin_id: PluginId,
        client_id: ClientId,
        worker_name: String,
        mut messages: Vec<(String, String)>,
    ) -> Result<()> {
        let worker =
            self.plugin_map
                .lock()
                .unwrap()
                .worker_sender(plugin_id, client_id, &worker_name);
        match worker {
            Some(worker) => {
                for (message, payload) in messages.drain(..) {
                    if let Err(e) = worker.send(MessageToWorker::Message(message, payload)) {
                        log::error!("Failed to send message to worker: {:?}", e);
                    }
                }
            },
            None => {
                log::warn!("Worker {worker_name} not found, caching messages");
                for (message, payload) in messages.drain(..) {
                    self.cached_worker_messages
                        .entry(plugin_id)
                        .or_default()
                        .push((client_id, worker_name.clone(), message, payload));
                }
            },
        }
        Ok(())
    }
    /// Grant `permissions` to the plugin at `plugin_url`, on top of whatever it already has.
    ///
    /// The cache is keyed the way `answer_permissions` keys it - by the location's string
    /// form - so the plugin's own later `request_permission` finds the grant and never prompts.
    /// An instance already running is told too, with the same event a prompt's answer sends,
    /// so it does not have to be reloaded to use what it was given.
    pub fn grant_permissions_to_location(
        &mut self,
        plugin_url: &str,
        permissions: Vec<PermissionType>,
        shutdown_sender: Sender<()>,
    ) -> Result<()> {
        let err_context = || format!("Failed to grant permissions to {plugin_url}");
        let location = yinz_runtime::plugin_location::parse_plugin_location(plugin_url, None)
            .map_err(|e| anyhow!("{}", e))
            .with_context(err_context)?;
        let location = self
            .normalize_plugin(RunPlugin {
                location,
                ..Default::default()
            })
            .location;
        let key = location.to_string();

        let mut permission_cache = PermissionCache::from_path_or_default(None);
        permission_cache.grant(key.clone(), permissions);
        permission_cache.write_to_file().with_context(err_context)?;
        let granted = permission_cache
            .get_permissions(key)
            .cloned()
            .unwrap_or_default();

        let running: Vec<PluginId> = self
            .plugin_map
            .lock()
            .unwrap()
            .running_plugins()
            .into_iter()
            .filter_map(|(plugin_id, _client_id, running_plugin)| {
                let mut running_plugin = running_plugin.lock().unwrap();
                let env = running_plugin.env();
                let loaded_location = match (&env.plugin_config.location, &env.resolved_source_path)
                {
                    (RunPluginLocation::File(_), Some(path)) => {
                        RunPluginLocation::File(path.clone())
                    },
                    (location, _) => {
                        self.normalize_plugin(RunPlugin {
                            location: location.clone(),
                            ..Default::default()
                        })
                        .location
                    },
                };
                if loaded_location != location {
                    return None;
                }
                running_plugin
                    .env_mut()
                    .set_permissions(HashSet::from_iter(granted.clone()));
                Some(plugin_id)
            })
            .collect();
        let tokens: Vec<_> = self
            .permission_requests
            .iter()
            .filter(|(_, pending)| {
                running.iter().any(|id| pending.contains_plugin(*id))
                    && pending
                        .permissions
                        .iter()
                        .all(|permission| granted.contains(permission))
            })
            .map(|(token, _)| *token)
            .collect();
        for token in tokens {
            self.answer_permissions(
                token,
                PermissionStatus::Granted,
                None,
                shutdown_sender.clone(),
            )?;
        }
        Ok(())
    }
    pub fn cache_plugin_events(&mut self, plugin_id: PluginId) {
        self.plugin_ids_waiting_for_permission_request
            .insert(plugin_id);
        self.cached_events_for_pending_plugins
            .entry(plugin_id)
            .or_default();
    }

    // Finds running or loading recipients, starting one if missing and allow_load is true.
    pub fn get_or_load_plugins(
        &mut self,
        run_plugin_or_alias: RunPluginOrAlias,
        size: Size,
        cwd: Option<PathBuf>,
        skip_cache: bool,
        should_float: bool,
        should_be_open_in_place: bool,
        pane_title: Option<String>,
        pane_id_to_replace: Option<PaneId>,
        cli_client_id: Option<ClientId>,
        floating_pane_coordinates: Option<FloatingPaneCoordinates>,
        should_focus: bool,
        allow_load: bool,
    ) -> Result<Vec<(PluginId, Option<ClientId>)>> {
        let run_plugin = run_plugin_or_alias.get_run_plugin();
        match run_plugin {
            Some(run_plugin) => {
                let all_plugin_ids = self.all_plugin_and_client_ids_for_plugin_location(
                    &run_plugin.location,
                    &run_plugin.configuration,
                );
                if all_plugin_ids.is_empty() {
                    if let Some(loading_plugin_id) = self.plugin_id_of_loading_plugin(
                        &run_plugin.location,
                        &run_plugin.configuration,
                    ) {
                        return Ok(vec![(loading_plugin_id, None)]);
                    }
                    if !allow_load {
                        return Ok(vec![]);
                    }
                    let headless = crate::host_bridge::is_workflow_bridge(&run_plugin.location);
                    let pending_id = PluginId::new(self.next_plugin_id);
                    if headless {
                        self.plugin_roles.lock().unwrap().insert(pending_id, true);
                    }
                    match self.load_plugin(
                        &Some(run_plugin),
                        None,
                        size,
                        cwd.clone(),
                        skip_cache,
                        cli_client_id,
                    ) {
                        // The bridge is a service with no UI: load it headless, as a suppressed
                        // pane, which also outlives the tab it was first addressed from.
                        Ok((plugin_id, client_id)) if headless => {
                            drop(
                                self.senders
                                    .send_to_screen(crate::add_suppressed_plugin_pane(
                                        run_plugin_or_alias,
                                        plugin_id,
                                        cwd,
                                        client_id,
                                    )),
                            );
                            Ok(vec![(plugin_id, Some(client_id))])
                        },
                        Ok((plugin_id, client_id)) => {
                            let start_suppressed = false;
                            drop(self.senders.send_to_screen(ScreenInstruction::AddPlugin(
                                Some(should_float),
                                should_be_open_in_place,
                                false, // close_replaced_pane
                                run_plugin_or_alias,
                                pane_title,
                                None,
                                plugin_id,
                                pane_id_to_replace,
                                cwd,
                                start_suppressed,
                                floating_pane_coordinates,
                                Some(should_focus),
                                Some(client_id),
                                None,
                                None,
                            )));
                            Ok(vec![(plugin_id, Some(client_id))])
                        },
                        Err(error) => {
                            self.plugin_roles.lock().unwrap().remove(&pending_id);
                            Err(error).context("Failed to load plugin")
                        },
                    }
                } else {
                    Ok(all_plugin_ids)
                }
            },
            None => Err(anyhow!(
                "Plugin not found for alias: {}",
                run_plugin_or_alias.location_string()
            )),
        }
    }
    pub fn pipe_has_recipient(&self, plugin: PluginId) -> bool {
        self.pending_pipes.has_recipient(plugin)
    }

    pub fn finish_empty_pipe(&mut self, stream: &str, result: Result<(), String>) {
        let request = self.pending_pipes.begin(stream);
        if let Err(error) = result {
            self.pending_pipes.fail(request, error);
        }
        self.finish_pipe_requests();
    }

    fn finish_pipe_requests(&mut self) {
        let outcomes = self.pending_pipes.take_ready();
        self.send_pipe_outcomes(outcomes);
    }

    pub(crate) fn send_pipe_outcomes(&self, outcomes: PipeOutcomes) {
        for (stream, result) in outcomes {
            let instruction = match result {
                Ok(()) => ServerInstruction::UnblockCliPipeInput(stream),
                Err(error) => ServerInstruction::CliPipeError(stream, error),
            };
            self.senders
                .send_to_server(instruction)
                .context("failed to report pipe outcome")
                .non_fatal();
        }
    }

    pub fn release_plugin_instance_pipes(
        &mut self,
        plugin: PluginId,
        instance: PluginInstanceId,
    ) -> PipeOutcomes {
        self.pending_pipes.retire(
            plugin,
            instance,
            "Plugin instance retired before completing its pipe",
        );
        self.pending_pipes.take_ready()
    }

    pub fn update_cli_pipe_state(&mut self, completions: Vec<PipeCompletion>) -> PipeOutcomes {
        for completion in completions {
            self.pending_pipes.complete(completion);
        }
        self.pending_pipes.take_ready()
    }
    fn message_is_directed_at_plugin(
        message_pid: Option<PluginId>,
        message_cid: Option<ClientId>,
        plugin_id: &PluginId,
        client_id: &ClientId,
        plugin_is_shared: bool,
    ) -> bool {
        // a shared plugin has a single instance for the whole session, so messages directed at any
        // client reach it (the message's client becomes the acting client)
        let client_matches = plugin_is_shared || message_cid == Some(*client_id);
        message_pid.is_none() && message_cid.is_none()
            || (message_pid.is_none() && client_matches)
            || (message_cid.is_none() && message_pid == Some(*plugin_id))
            || (client_matches && message_pid == Some(*plugin_id))
    }
    pub fn client_is_connected(&self, client_id: &ClientId) -> bool {
        self.connected_clients.lock().unwrap().contains(client_id)
    }
    pub fn get_first_client_id(&self) -> Option<ClientId> {
        self.connected_clients
            .lock()
            .unwrap()
            .iter()
            .next()
            .copied()
    }
    pub fn update_available_layouts(
        &mut self,
        layouts: Vec<LayoutInfo>,
        errors: Vec<LayoutWithError>,
    ) {
        // Diff with existing layouts
        if self.available_layouts != layouts || self.available_layout_errors != errors {
            // Update the stored layouts
            self.available_layouts = layouts.clone();
            self.available_layout_errors = errors.clone();

            // Notify all plugins of the change
            let _ = self.senders.send_to_plugin(PluginInstruction::Update(vec![(
                None, // Broadcast to all plugins
                None, // Broadcast to all clients
                Event::AvailableLayoutInfo(layouts, errors),
            )]));
        }
    }
    pub fn state_update_for_plugin(&self, plugin_id: PluginId) {
        let _ = self.senders.send_to_plugin(PluginInstruction::Update(vec![(
            Some(plugin_id),
            None,
            Event::AvailableLayoutInfo(
                self.available_layouts.clone(),
                self.available_layout_errors.clone(),
            ),
        )]));
    }
    pub fn detect_and_notify_plugin_config_changes(
        &mut self,
        new_plugins: &PluginAliases,
        shutdown_send: Sender<()>,
    ) -> Result<()> {
        let err_context = || "failed to detect plugin config changes";

        // Before notifying anyone: this is the only place the thread is handed a fresh
        // alias table, and `GetPluginAliasConfiguration` answers out of it. Without this
        // a plugin asking after a config reload would be told what was on disk when the
        // server started.
        *self.plugin_aliases.lock().unwrap() = new_plugins.clone();

        // Get all running plugins
        let running_plugins = self.plugin_map.lock().unwrap().running_plugins();

        for (plugin_id, client_id, running_plugin) in running_plugins {
            let mut running_plugin = running_plugin.lock().unwrap();
            let plugin_env = &running_plugin.env();
            let plugin_location = &plugin_env.plugin_config.location;

            // Look up this plugin in the new config by location
            // Note: PluginAliases is HashMap<String, RunPlugin>, so we need to iterate
            let new_config_for_location = new_plugins
                .aliases
                .values()
                .find(|run_plugin| {
                    self.normalize_plugin((*run_plugin).clone()).location
                        == self
                            .normalize_plugin(RunPlugin {
                                location: plugin_location.clone(),
                                ..Default::default()
                            })
                            .location
                })
                .map(|run_plugin| &run_plugin.configuration);

            if let Some(new_config) = new_config_for_location {
                // Compare configurations - only fire event if changed
                if &running_plugin.current_configuration != new_config {
                    running_plugin.current_configuration = new_config.clone();
                    drop(running_plugin); // Release lock before sending

                    let event = Event::PluginConfigurationChanged(new_config.inner().clone());
                    let updates = vec![(Some(plugin_id), Some(client_id), event)];
                    self.update_plugins(updates, shutdown_send.clone())
                        .with_context(err_context)?;
                }
            }
        }

        Ok(())
    }
}

/// The loader's first move, with the `PluginMap` lock held: if the plugin was unloaded while it
/// was still loading, tidy up what `load_plugin` set in motion and report that there is nothing
/// to do. See `WasmBridge::unloaded_while_loading` for why the lock matters.
fn load_was_cancelled(
    unloaded_while_loading: &Arc<Mutex<HashSet<PluginId>>>,
    plugin_id: PluginId,
    senders: &ThreadSenders,
    plugin_executor: &Arc<PinnedExecutor>,
) -> bool {
    if !unloaded_while_loading.lock().unwrap().remove(&plugin_id) {
        return false;
    }
    log::info!("plugin {plugin_id} was unloaded before it finished loading, not starting it");
    let _ = senders.send_to_background_jobs(BackgroundJob::StopPluginLoadingAnimation(plugin_id));
    plugin_executor.unregister_plugin(plugin_id);
    true
}

fn handle_plugin_successful_loading(
    senders: &ThreadSenders,
    plugin_id: PluginId,
    plugin_list: BTreeMap<PluginId, RunPlugin>,
) {
    let _ = senders.send_to_background_jobs(BackgroundJob::StopPluginLoadingAnimation(plugin_id));
    let _ = senders.send_to_screen(ScreenInstruction::RequestStateUpdateForPlugins);
    let _ = senders.send_to_background_jobs(BackgroundJob::ReportPluginList(plugin_list));
    let _ = senders.send_to_plugin(PluginInstruction::RequestStateUpdateForPlugin(plugin_id));
}

/// A short label for the executor's stats log — the batch's event kinds with
/// counts, e.g. "Key" or "PaneUpdate×2,TabUpdate" — and whether any event in it
/// is user input (a focused or intercepted key, a mouse action, a paste), which
/// is what marks the job urgent for the executor's accounting.
fn summarize_batch_for_stats<'a>(events: impl IntoIterator<Item = &'a Event>) -> (String, bool) {
    let mut counts: Vec<(EventType, usize)> = Vec::new();
    let mut has_input = false;
    for event in events {
        let event_type = EventType::from(event);
        has_input |= matches!(
            event_type,
            EventType::Key
                | EventType::Mouse
                | EventType::PastedText
                | EventType::InterceptedKeyPress
        );
        match counts.iter_mut().find(|(kind, _)| *kind == event_type) {
            Some((_, count)) => *count += 1,
            None => counts.push((event_type, 1)),
        }
    }
    let label = counts
        .iter()
        .map(|(kind, count)| match count {
            1 => kind.to_string(),
            _ => format!("{kind}×{count}"),
        })
        .collect::<Vec<_>>()
        .join(",");
    (label, has_input)
}

fn handle_plugin_loading_failure(
    senders: &ThreadSenders,
    load_id: Option<u64>,
    plugin_id: PluginId,
    loading_indication: &mut LoadingIndication,
    error: impl std::fmt::Debug,
    client_id: Option<ClientId>,
) {
    log::error!("{:?}", error);
    let _ = senders.send_to_background_jobs(BackgroundJob::StopPluginLoadingAnimation(plugin_id));
    let _ = senders.send_to_plugin(PluginInstruction::PluginFailed {
        load_id,
        plugin_id,
        deno_generation: None,
        instance_id: None,
        loading_client: client_id,
        error: format!("{:?}", error),
        failed_to_load: true,
    });
    loading_indication.indicate_loading_error(format!("{:?}", error));
    let _ = senders.send_to_screen(ScreenInstruction::UpdatePluginLoadingStage(
        plugin_id,
        loading_indication.clone(),
    ));
}

pub(crate) fn check_event_permission(
    plugin_env: &PluginEnv,
    event: &Event,
) -> (PermissionStatus, Option<PermissionType>) {
    if plugin_env.plugin_config.is_builtin() {
        // built-in plugins can do all the things because they're part of the application and
        // there's no use to deny them anything
        return (PermissionStatus::Granted, None);
    }
    // Matched on the discriminant rather than on `Event`, which is `#[non_exhaustive]` and so
    // could only ever be matched with a wildcard from this crate. On `EventType` the match is
    // exhaustive: a new event that is not classified here is a compile error rather than an
    // event delivered to every plugin regardless of what it has been granted.
    let permission = match EventType::from(event) {
        EventType::ModeUpdate
        | EventType::TabUpdate
        | EventType::PaneTabsUpdate
        | EventType::PaneUpdate
        | EventType::SessionUpdate
        | EventType::CopyToClipboard
        | EventType::SystemClipboardFailure
        | EventType::CommandPaneOpened
        | EventType::CommandPaneExited
        | EventType::PaneClosed
        | EventType::EditPaneOpened
        | EventType::EditPaneExited
        | EventType::FailedToWriteConfigToDisk
        | EventType::CommandPaneReRun
        | EventType::CwdChanged
        | EventType::CommandChanged
        | EventType::AvailableLayoutInfo
        | EventType::PluginConfigurationChanged
        | EventType::HighlightClicked
        | EventType::HintText
        | EventType::ActivePaneScroll
        | EventType::InputReceived
        // Which modifier keys are down says something about what the person at the keyboard is
        // doing, which is more than `InputReceived`'s bare "something happened" - so it sits
        // with the rest of the session state rather than with the ungated group below.
        | EventType::ModifiersHeld => PermissionType::ReadApplicationState,
        EventType::PluginDataChanged => PermissionType::PluginData,
        // What an agent said and was asked is session state, and more of it than the manifest
        // carries: a plugin that may read the panes may read this.
        EventType::AgentEvent => PermissionType::ReadApplicationState,
        EventType::PickResult => PermissionType::MessageAndLaunchOtherPlugins,
        EventType::AgentProvidersChanged => PermissionType::RunCommands,
        // Other plugins' jobs are session state. A plugin's own jobs' Started/Finished events
        // need nothing more than the permission it enqueued with.
        EventType::NotificationReceived | EventType::NotificationResolved => PermissionType::ReadApplicationState,
        EventType::QueueChanged => PermissionType::ReadApplicationState,
        EventType::PaneOutput | EventType::PaneRenderReport => PermissionType::ReadPaneContents,
        EventType::UserAction => PermissionType::InterceptInput,
        // Ungated on purpose. These are either addressed to this plugin alone (its own input,
        // timers, lifecycle, worker messages and the replies to commands it was already
        // permitted to issue) or carry nothing about the session a plugin could not learn by
        // being loaded into it.
        EventType::Key
        | EventType::Mouse
        | EventType::PastedText
        | EventType::InterceptedKeyPress
        | EventType::Timer
        | EventType::Visible
        | EventType::ClientGridReset
        | EventType::BeforeClose
        | EventType::CustomMessage
        | EventType::PermissionRequestResult
        | EventType::InitialKeybinds
        | EventType::HostTerminalThemeChanged
        | EventType::ActionComplete
        | EventType::RunCommandOutput
        | EventType::RunCommandResult
        | EventType::WebRequestResult
        | EventType::WebRequestStreamChunk
        | EventType::ListClients
        | EventType::HostFolderChanged
        | EventType::FailedToChangeHostFolder
        | EventType::ConfigWasWrittenToDisk
        | EventType::NotificationRequested
        | EventType::ContextMenuRequested
        | EventType::ContextMenuItemSelected
        // Never broadcast: `deliver_to_headless_hook` addresses it to the one instance of the
        // plugin the user bound in `hooks { }`, and binding it is the consent. A headless plugin
        // has no pane to show a permission prompt on, so gating this would mean silence.
        | EventType::Hook
        | EventType::QueueJobStarted
        | EventType::QueueJobFinished
        // Paths and metadata under the host folder, not contents; see `ScanHostFolder` in
        // `yinz_exports::command_permission` for why these stay open for now.
        | EventType::FileSystemCreate
        | EventType::FileSystemRead
        | EventType::FileSystemUpdate
        | EventType::FileSystemDelete => return (PermissionStatus::Granted, None),
    };

    if let Some(permissions) = plugin_env.permissions.lock().unwrap().as_ref() {
        if permissions.contains(&permission) {
            return (PermissionStatus::Granted, None);
        }
    }

    (PermissionStatus::Denied, Some(permission))
}

/// Call the plugin's `render` once per client, yielding one render asset per client. For a
/// non-shared plugin `clients` is just the client owning this instance. For a shared plugin it's
/// every connected client - the plugin's acting client id is set before each call so that it may
/// vary its output per client.
///
/// ponytail: N wasm render calls for a shared plugin. If a plugin's output never depends on the
/// client, render once and clone the bytes instead.
pub fn render_plugin_for_clients(
    plugin_id: PluginId,
    running_plugin: &mut RunningPlugin,
    clients: &[ClientId],
) -> Result<Vec<PluginRenderAsset>> {
    if running_plugin.is_disposed() {
        return Ok(vec![]);
    }
    let err_context = || format!("Failed to render plugin {plugin_id}");
    // The newest size the screen asked for, not the last one a resize job applied: a resize
    // can still be queued behind this render, or have been skipped for a newer one.
    let (rows, columns) = running_plugin.take_size_for_render();
    let owner_client_id = running_plugin.env().owner_client_id;
    // Guest rendering still runs while detached: it may issue host commands.
    let mut clients_to_render = vec![owner_client_id];
    clients_to_render.extend(clients.iter().filter(|c| **c != owner_client_id));

    let mut plugin_render_assets = vec![];
    for client_id in clients_to_render {
        running_plugin.env_mut().client_id = client_id;
        let rendered_bytes = running_plugin
            .render(rows, columns)
            .with_context(err_context)?;
        plugin_render_assets.push(PluginRenderAsset::new(plugin_id, client_id, rendered_bytes));
    }
    Ok(plugin_render_assets)
}

/// Deliver a batch of events to one plugin instance, in order, then render it once if any of
/// them asked for it. Each event carries the client it acts for, which for a shared plugin
/// differs from the instance's owner.
///
/// Rendering once per batch rather than once per event is what keeps a flood of wheel notches
/// from costing a wasm `render()` per client per notch; the plugin still sees every event. A
/// plugin that errors mid-batch is not handed the rest of it, but the CLI pipe changes the
/// events before the error asked for are still delivered: a `yinz pipe` client one of them
/// released must not stay blocked on a later event's failure.
pub fn apply_events_to_plugin(
    plugin_id: PluginId,
    render_clients: &[ClientId],
    running_plugin: &mut RunningPlugin,
    events: &[(ClientId, Event)],
    plugin_render_assets: &mut Vec<PluginRenderAsset>,
    senders: ThreadSenders,
    plugin_subscriptions: &HashSet<EventType>,
) -> Result<()> {
    if running_plugin.is_unavailable() {
        return Ok(());
    }
    let err_context = || format!("Failed to apply event to plugin {plugin_id}");

    let mut should_render = false;
    let mut handled_any = false;
    let mut failure: Option<anyhow::Error> = None;
    // The pipe changes each event asked for, read off the plugin as soon as that event returns.
    // One drain at the end of the batch would merge them with unblock winning over block
    // regardless of order (see `pipes_to_block_or_unblock`); drained per event, the last change
    // to a pipe wins, as it did when every event was its own job.
    let mut pipe_changes: HashMap<String, PipeStateChange> = HashMap::new();
    for (client_id, event) in events {
        // host commands issued while handling this event act on behalf of the client it was
        // directed at (this only differs from the instance's owner for shared plugins)
        running_plugin.env_mut().client_id = *client_id;
        match check_event_permission(running_plugin.env(), event) {
            (PermissionStatus::Granted, _) => {
                let mut event = event.clone();
                // Registrations can change on different executor threads after a broadcast
                // queues. Mirror the current registry, never an older queued snapshot.
                if let Event::AgentProvidersChanged(providers) = &mut event {
                    *providers = yinz_types::agent::provider::providers();
                }
                if let Event::ModeUpdate(mode_info) = &mut event {
                    // A ModeUpdate that already names its base mode (Screen::change_mode sets it
                    // when a keybinding preset switches) must keep it: the plugin's env is updated
                    // asynchronously on reconfigure and may still hold the previous default mode.
                    if mode_info.base_mode.is_none() {
                        mode_info.base_mode = Some(running_plugin.env().default_mode);
                    }
                    if plugin_subscriptions.contains(&EventType::InitialKeybinds) {
                        // Plugin caches keybindings via InitialKeybinds — send lightweight ModeUpdate
                        mode_info.keybinds = vec![];
                    } else {
                        // Legacy plugin — send full keybindings as before
                        mode_info.keybinds = running_plugin.env().keybinds.to_keybinds_vec();
                    }
                }
                if let Event::NotificationRequested(notification) = &event {
                    senders
                        .send_to_screen(ScreenInstruction::BindNotificationRequest {
                            id: notification.id,
                            plugin_id,
                            instance_id: running_plugin.env().instance_id,
                        })
                        .context("bind notification policy request")
                        .non_fatal();
                }
                handled_any = true;
                let updated = running_plugin.update(&event).with_context(err_context);
                let changes = pipes_to_block_or_unblock(running_plugin);
                match updated {
                    Ok(rendered) => {
                        should_render |= rendered;
                        pipe_changes.extend(changes);
                    },
                    Err(e) => {
                        failure = Some(e);
                        break;
                    },
                }
                if let Event::PermissionRequestResult(..) = event {
                    // we always render in this case, otherwise the request permission screen stays on
                    // screen
                    should_render = true;
                }
            },
            (PermissionStatus::Denied, permission) => {
                // Once per plugin per event type: a plugin subscribed to something it may not
                // read is a standing misconfiguration, and an event as frequent as
                // `ModifiersHeld` would otherwise fill the log with the same line.
                let event_type = EventType::from(event);
                if running_plugin.note_denied_event(event_type) {
                    log::error!(
                        "PluginId '{}' permission '{}' is not allowed - Event '{:?}' denied \
                         (further denials of this event are not logged)",
                        plugin_id,
                        permission
                            .map(|p| p.to_string())
                            .unwrap_or("UNKNOWN".to_owned()),
                        event_type
                    );
                }
            },
        }
    }
    if !handled_any {
        return Ok(());
    }
    let mut rendered = None;
    let mut render_failed = false;
    if failure.is_none() && should_render && running_plugin.has_area() {
        match render_plugin_for_clients(plugin_id, running_plugin, render_clients) {
            Ok(assets) => rendered = Some(assets),
            Err(e) => {
                render_failed = true;
                failure = Some(e);
            },
        }
    }
    if let Some(mut assets) = rendered {
        plugin_render_assets.append(&mut assets);
    }
    if !render_failed {
        report_pipe_changes(running_plugin, pipe_changes, None, &senders);
    }
    if failure.is_some() {
        running_plugin.mark_failed();
    }
    failure.map_or(Ok(()), Err)
}

/// How long an unload gives the plugin's current entrypoint call to return before the plugin is
/// cancelled. Its cleanup job is queued on its own pinned thread, behind that call; a call that
/// has not returned in this long is not coming back on its own.
const UNLOAD_GRACE: Duration = Duration::from_secs(3);

/// Set `cancel` after `UNLOAD_GRACE` if one of the plugin's `instances` is still inside an
/// entrypoint by then.
///
/// "Inside an entrypoint" is read off the instance's lock, which the executor holds for exactly
/// as long as the guest runs. Not off whether the cleanup job has run: executor threads are
/// shared, so an idle instance whose cleanup is merely queued behind another plugin's long call
/// would look stuck and be cancelled for nothing — and a flag the cleanup job sets races the
/// job's own reset of the token. A lock that is free means the guest is not running, whether
/// its cleanup ran or is still queued; one that is held means it is, and that call is what the
/// grace was for. (Held by a *later* entrypoint queued before the cleanup is the one false
/// positive left: that instance loses its `BeforeClose`, which is being unloaded anyway.)
fn arm_unload_watchdog(
    plugin_id: PluginId,
    cancel: Arc<AtomicBool>,
    instances: Vec<Arc<Mutex<RunningPlugin>>>,
) {
    // Observing a call must not keep its disposed instance and temporary directory alive.
    let instances: Vec<_> = instances.iter().map(Arc::downgrade).collect();
    yinz_bus::global_async_runtime::get_tokio_runtime().spawn(async move {
        tokio::time::sleep(UNLOAD_GRACE).await;
        if instances
            .iter()
            .filter_map(Weak::upgrade)
            .any(|instance| is_inside_entrypoint(&instance))
        {
            log::warn!(
                "plugin {plugin_id} has not returned from its entrypoint {UNLOAD_GRACE:?} after \
                 its unload; cancelling it"
            );
            cancel.store(true, Ordering::Relaxed);
        }
    });
}

/// `arm_unload_watchdog` for the instances a reload is about to replace, armed when the reload is
/// requested rather than when its job runs. Weak, so it does not outlive them: an instance the
/// reload job already dropped is not running.
fn arm_reload_watchdog(
    plugin_id: PluginId,
    cancel: Arc<AtomicBool>,
    instances: Vec<Weak<Mutex<RunningPlugin>>>,
) {
    yinz_bus::global_async_runtime::get_tokio_runtime().spawn(async move {
        tokio::time::sleep(UNLOAD_GRACE).await;
        if instances
            .iter()
            .filter_map(Weak::upgrade)
            .any(|instance| is_inside_entrypoint(&instance))
        {
            log::warn!(
                "plugin {plugin_id} has not returned from its entrypoint {UNLOAD_GRACE:?} after \
                 its reload; cancelling it"
            );
            cancel.store(true, Ordering::Relaxed);
        }
    });
}

/// Whether the guest is running right now: its executor holds the instance's lock for exactly
/// that long. A poisoned lock is a guest that panicked, which is not running.
fn is_inside_entrypoint(instance: &Arc<Mutex<RunningPlugin>>) -> bool {
    matches!(instance.try_lock(), Err(TryLockError::WouldBlock))
}

/// `mutex.lock()`, or with a deadline a `try_lock` loop that gives up as `None` once it passes.
fn lock_with_deadline<T>(mutex: &Mutex<T>, deadline: Option<Instant>) -> Option<MutexGuard<'_, T>> {
    let Some(deadline) = deadline else {
        return Some(mutex.lock().unwrap());
    };
    loop {
        match mutex.try_lock() {
            Ok(guard) => return Some(guard),
            Err(TryLockError::Poisoned(poisoned)) => return Some(poisoned.into_inner()),
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5));
            },
            Err(TryLockError::WouldBlock) => return None,
        }
    }
}

pub fn handle_plugin_crash(
    plugin_id: PluginId,
    deno_generation: Option<u64>,
    instance_id: Option<PluginInstanceId>,
    message: String,
    senders: ThreadSenders,
) {
    let _ = senders.send_to_plugin(PluginInstruction::PluginFailed {
        load_id: None,
        plugin_id,
        deno_generation,
        instance_id,
        loading_client: None,
        error: message.clone(),
        failed_to_load: false,
    });
}

fn show_plugin_crash(plugin_id: PluginId, message: String, senders: &ThreadSenders) {
    let mut loading_indication = LoadingIndication::new("Panic!".to_owned());
    loading_indication.indicate_loading_error(message);
    let _ = senders.send_to_screen(ScreenInstruction::UpdatePluginLoadingStage(
        plugin_id,
        loading_indication,
    ));
}

pub fn apply_before_close_event_to_plugin(
    plugin_id: PluginId,
    _client_id: ClientId,
    running_plugin: &mut RunningPlugin,
    senders: ThreadSenders,
) -> Result<()> {
    let err_context = || format!("Failed to apply event to plugin {plugin_id}");
    let _should_render = running_plugin
        .update(&Event::BeforeClose)
        .with_context(err_context)?;
    report_pipe_changes(running_plugin, HashMap::new(), None, &senders);
    Ok(())
}

/// The instances running at one location whose configuration is the one asked for.
///
/// **Compared without the caller context, not looked up by key.** The map is keyed on the whole
/// `PluginUserConfiguration`, and `caller_cwd` / `caller_terminal_pane_id` are stamped onto that
/// map by whoever asked for the launch rather than written by the user: the keybinding path writes
/// both (`Pty::spawn_plugin`), a plugin asking for another plugin by url writes only the first
/// (`yinz_exports::message_to_plugin`). An exact key lookup therefore failed to see a plugin
/// that was already running and started a second copy of it - which is how a second, hidden agent
/// conversation ended up behind the agent list, placing the list into the visible conversation's
/// slot on top of it.
///
/// `RunPluginOrAlias::is_equivalent_to_run` and `configuration_matches_for_reload` have always
/// asked the question this way. This was the one lookup that did not.
fn instances_of<'a>(
    at_this_location: &'a HashMap<PluginUserConfiguration, Vec<(PluginId, ClientId)>>,
    plugin_configuration: &PluginUserConfiguration,
) -> &'a [(PluginId, ClientId)] {
    let wanted = plugin_configuration.without_caller_context();
    at_this_location
        .iter()
        .find(|(configuration, _)| configuration.without_caller_context() == wanted)
        .map(|(_, instances)| instances.as_slice())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use {yinz_types::data::BareKey, yinz_types::data::KeyWithModifier};

    #[test]
    fn cached_deno_events_keep_generation_and_origin_across_attachment_and_reload() {
        let first = ClientId::new(1);
        let second = ClientId::new(2);
        let cached = [
            EventOrPipeMessage::DenoEvent(first, 41, Event::Visible(true), None),
            EventOrPipeMessage::DenoEvent(second, 42, Event::Visible(false), None),
        ];
        let replay = |owner, generation, shared| {
            cached
                .iter()
                .filter_map(|message| {
                    let client = message.client_for_replay(owner, generation, shared)?;
                    let EventOrPipeMessage::DenoEvent(_, _, event, _) = message else {
                        unreachable!()
                    };
                    Some((client, event.clone()))
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            replay(first, Some(41), false),
            vec![(first, Event::Visible(true))]
        );
        assert_eq!(
            replay(second, Some(42), false),
            vec![(second, Event::Visible(false))]
        );
        assert!(replay(second, Some(41), false).is_empty());
        assert!(replay(first, Some(43), false).is_empty());
        assert!(replay(first, None, false).is_empty());
        // Shared ownership may move, but commands resumed by this completion must
        // still act for the client which started the operation.
        assert_eq!(
            replay(second, Some(41), true),
            vec![(first, Event::Visible(true))]
        );
        assert_eq!(
            EventOrPipeMessage::Event(Event::InputReceived).client_for_replay(second, None, false),
            Some(second)
        );
    }

    fn plugin_config(configuration: &[(&str, &str)]) -> PluginConfig {
        let configuration = configuration
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        PluginConfig {
            repository_root: None,
            installed_commit: None,
            path: PathBuf::from("test-plugin"),
            _allow_exec_host_cmd: false,
            location: yinz_runtime::plugin_location::parse_plugin_location("yinz:tab-bar", None)
                .unwrap(),
            initial_userspace_configuration: PluginUserConfiguration::new(configuration),
            initial_cwd: None,
            relative_path: None,
        }
    }

    #[test]
    fn shared_plugins_are_opted_into_through_their_configuration() {
        assert!(!plugin_config(&[]).is_shared());
        assert!(plugin_config(&[("shared", "true")]).is_shared());
        assert!(!plugin_config(&[("shared", "false")]).is_shared());
        assert!(!plugin_config(&[("shared", "yes please")]).is_shared());
    }

    #[test]
    fn intercepted_keypress_batches_are_input_jobs() {
        let events = [(
            ClientId::new(1),
            Event::InterceptedKeyPress(KeyWithModifier::new(BareKey::Char('x'))),
        )];

        assert_eq!(
            summarize_batch_for_stats(events.iter().map(|(_, event)| event)),
            ("InterceptedKeyPress".to_owned(), true)
        );
    }

    /// The two halves of agent address each other by url, and the launch behind that url has to
    /// find the half that is already on screen rather than open a second one.
    ///
    /// The running instance carries the caller context the keybinding stamped on it; the greeting
    /// from the other half carries its own, and less of it. Neither is identity.
    #[test]
    fn caller_context_does_not_hide_a_running_plugin_from_the_launch_lookup() {
        let configuration = |pairs: &[(&str, &str)]| {
            PluginUserConfiguration::new(
                pairs
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            )
        };
        // as launched by the `c` keybinding, which goes through `Pty::spawn_plugin`
        let running = configuration(&[
            ("overlay", "true"),
            ("awaits_placement", "true"),
            ("shared", "true"),
            ("caller_cwd", "/repo"),
            ("caller_terminal_pane_id", "0"),
        ]);
        let at_this_location =
            HashMap::from([(running, vec![(PluginId::new(7), ClientId::new(1))])]);

        // as asked for by `agents`, whose message is stamped with `caller_cwd` alone
        let greeting = configuration(&[
            ("overlay", "true"),
            ("awaits_placement", "true"),
            ("shared", "true"),
            ("caller_cwd", "/somewhere/else"),
        ]);
        assert_eq!(
            instances_of(&at_this_location, &greeting),
            &[(PluginId::new(7), ClientId::new(1))],
            "the conversation that is already open"
        );

        // and a configuration that differs by something the user actually wrote is still a
        // different plugin
        let other = configuration(&[("overlay", "true"), ("shared", "true")]);
        assert!(
            instances_of(&at_this_location, &other).is_empty(),
            "an overlay that does not await placement is not the same plugin"
        );
    }

    #[test]
    fn shared_plugins_receive_messages_directed_at_any_client() {
        let (plugin_id, owner_client_id, other_client_id) = (1, 1, 2);
        // a message for another client reaches a shared plugin, but not a per-client one
        assert!(WasmBridge::message_is_directed_at_plugin(
            Some(PluginId::new(plugin_id)),
            Some(ClientId::new(other_client_id)),
            &PluginId::new(plugin_id),
            &ClientId::new(owner_client_id),
            true,
        ));
        assert!(!WasmBridge::message_is_directed_at_plugin(
            Some(PluginId::new(plugin_id)),
            Some(ClientId::new(other_client_id)),
            &PluginId::new(plugin_id),
            &ClientId::new(owner_client_id),
            false,
        ));
        // a message for another plugin never reaches it
        assert!(!WasmBridge::message_is_directed_at_plugin(
            Some(PluginId::new(plugin_id + 1)),
            Some(ClientId::new(other_client_id)),
            &PluginId::new(plugin_id),
            &ClientId::new(owner_client_id),
            true,
        ));
        // broadcasts still reach it exactly as before
        assert!(WasmBridge::message_is_directed_at_plugin(
            None,
            None,
            &PluginId::new(plugin_id),
            &ClientId::new(owner_client_id),
            true,
        ));
    }

    /// The other side of the rule above: a toast outlives the turn that raised it, so the client
    /// who clicks one is not always the client whose instance put it up. The single id an update
    /// carries has to be the raiser to *reach* a per-client plugin, and the clicker to *act* as
    /// the right person on a shared one - and one choice cannot be both.
    #[test]
    fn a_notification_click_names_the_raiser_only_when_that_is_what_reaches_it() {
        let (raiser, clicker) = (ClientId::new(2), ClientId::new(1));

        // shared: every client reaches the one instance, so the id is spent on the acting client
        assert_eq!(
            crate::client_for_notification_click(true, raiser, clicker),
            clicker,
            "the reveal belongs to whoever clicked"
        );
        // per-client: only the raiser's instance knows about the toast, and naming anybody else
        // drops the click silently
        assert_eq!(
            crate::client_for_notification_click(false, raiser, clicker),
            raiser,
        );
        assert!(WasmBridge::message_is_directed_at_plugin(
            Some(PluginId::new(1)),
            Some(crate::client_for_notification_click(false, raiser, clicker)),
            &PluginId::new(1),
            &raiser,
            false,
        ));

        // and the usual case - one client - is the same id either way
        assert_eq!(
            crate::client_for_notification_click(true, clicker, clicker),
            crate::client_for_notification_click(false, clicker, clicker),
        );
    }
}

#[path = "./unit/wasm_bridge_tests.rs"]
#[cfg(test)]
mod wasm_bridge_tests;
