#![allow(clippy::type_complexity)]
use crate::plugins::lua::LuaPlugin;
use crate::plugins::plugin_limits::PluginResourceLimits;
use crate::plugins::plugin_worker::MessageToWorker;
use crate::plugins::zellij_exports::{wasi_read_string, wasi_write_object};
use crate::plugins::PluginId;
use std::io::Write;
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc, Mutex},
};
use wasmi::{Instance, Store};
use wasmi_wasi::WasiCtx;

use crate::{thread_bus::ThreadSenders, ClientId};

use prost::Message;
use tokio::sync::mpsc::UnboundedSender;
use zellij_utils::data_access::DataAccess;
use zellij_utils::plugin_api::action::ProtobufPluginConfiguration;
use zellij_utils::plugin_api::event::ProtobufEvent;
use zellij_utils::plugin_api::pipe_message::ProtobufPipeMessage;

use crate::plugins::queues::Queues;
use crate::plugins::wasm_bridge::WasmBridge;
use zellij_utils::{
    data::EventType,
    data::InputMode,
    input::command::TerminalAction,
    input::keybinds::Keybinds,
    input::layout::{PluginUserConfiguration, RunPlugin, RunPluginLocation},
    input::plugins::{PluginAliases, PluginConfig},
};
use zellij_utils::{
    data::{Event, PermissionType, PipeMessage},
    errors::prelude::*,
};

// the idea here is to provide atomicity when adding/removing plugins from the map (eg. when a new
// client connects) but to also allow updates/renders not to block each other
// so when adding/removing from the map - everything is halted, that's life
// but when cloning the internal RunningPlugin and Subscriptions atomics, we can call methods on
// them without blocking other instances
#[derive(Default)]
pub struct PluginMap {
    plugin_assets: HashMap<
        (PluginId, ClientId),
        (
            Arc<Mutex<RunningPlugin>>,
            Arc<Mutex<Subscriptions>>,
            HashMap<String, UnboundedSender<MessageToWorker>>,
        ),
    >,
}

/// Configuration match used when looking up running plugins to reload.
///
/// Deliberately looser than plain equality: caller context is injected into an alias's
/// configuration per-launch (`Pty::spawn_plugin`), so it is never part of a plugin's identity.
/// `RunPluginOrAlias::is_equivalent_to_run` ignores it for the same reason. And an empty request
/// (eg. `zellij action start-or-reload-plugin zellij:hocus`, which carries none of the alias'
/// configuration) means "whatever is running at this location", otherwise reloading a
/// configured plugin by its bare location would start a second copy of it instead.
fn configuration_matches_for_reload(
    running: &PluginUserConfiguration,
    requested: &PluginUserConfiguration,
) -> bool {
    let without_launch_context = |configuration: &PluginUserConfiguration| {
        let mut configuration = configuration.without_caller_context();
        // A queued job's identity keys, added per launch by `WasmBridge::launch_job`. Stripped
        // here rather than in `without_caller_context`, which every "is this the plugin that is
        // already running?" comparison shares: a queued job *is* a distinct instance everywhere
        // else, and only a reload lookup wants to see past that.
        configuration.remove(WasmBridge::QUEUE_CONFIG_KEY);
        configuration.remove(WasmBridge::QUEUE_JOB_ID_CONFIG_KEY);
        configuration.remove(WasmBridge::QUEUE_ENQUEUER_CONFIG_KEY);
        configuration
    };
    let requested = without_launch_context(requested);
    requested.is_empty() || without_launch_context(running) == requested
}

impl PluginMap {
    pub fn remove_plugins(
        &mut self,
        pid: PluginId,
    ) -> HashMap<
        (PluginId, ClientId),
        (
            Arc<Mutex<RunningPlugin>>,
            Arc<Mutex<Subscriptions>>,
            HashMap<String, UnboundedSender<MessageToWorker>>,
        ),
    > {
        let mut removed = HashMap::new();
        let ids_in_plugin_map: Vec<(PluginId, ClientId)> =
            self.plugin_assets.keys().copied().collect();
        for (plugin_id, client_id) in ids_in_plugin_map {
            if pid == plugin_id {
                if let Some(plugin_asset) = self.plugin_assets.remove(&(plugin_id, client_id)) {
                    removed.insert((plugin_id, client_id), plugin_asset);
                }
            }
        }
        removed
    }
    pub fn plugin_ids(&self) -> Vec<PluginId> {
        let mut unique_plugins: HashSet<PluginId> = self
            .plugin_assets
            .keys()
            .map(|(plugin_id, _client_id)| *plugin_id)
            .collect();
        unique_plugins.drain().collect()
    }
    pub fn running_plugins(&mut self) -> Vec<(PluginId, ClientId, Arc<Mutex<RunningPlugin>>)> {
        self.plugin_assets
            .iter()
            .map(|((plugin_id, client_id), (running_plugin, _, _))| {
                (*plugin_id, *client_id, running_plugin.clone())
            })
            .collect()
    }
    pub fn running_plugins_and_subscriptions(
        &mut self,
    ) -> Vec<(
        PluginId,
        ClientId,
        Arc<Mutex<RunningPlugin>>,
        Arc<Mutex<Subscriptions>>,
    )> {
        self.plugin_assets
            .iter()
            .map(
                |((plugin_id, client_id), (running_plugin, subscriptions, _))| {
                    (
                        *plugin_id,
                        *client_id,
                        running_plugin.clone(),
                        subscriptions.clone(),
                    )
                },
            )
            .collect()
    }
    pub fn get_running_plugin_and_subscriptions(
        &self,
        plugin_id: PluginId,
        client_id: ClientId,
    ) -> Option<(Arc<Mutex<RunningPlugin>>, Arc<Mutex<Subscriptions>>)> {
        self.plugin_assets
            .get(&(plugin_id, client_id))
            .map(|(running_plugin, subscriptions, _)| {
                (running_plugin.clone(), subscriptions.clone())
            })
    }
    pub fn get_running_plugin(
        &self,
        plugin_id: PluginId,
        client_id: Option<ClientId>,
    ) -> Option<Arc<Mutex<RunningPlugin>>> {
        match client_id {
            Some(client_id) => self
                .plugin_assets
                .get(&(plugin_id, client_id))
                .map(|(running_plugin, _, _)| running_plugin.clone()),
            None => self
                .plugin_assets
                .iter()
                .find(|((p_id, _), _)| *p_id == plugin_id)
                .map(|(_, (running_plugin, _, _))| running_plugin.clone()),
        }
    }
    pub fn worker_sender(
        &self,
        plugin_id: PluginId,
        client_id: ClientId,
        worker_name: &str,
    ) -> Option<UnboundedSender<MessageToWorker>> {
        self.plugin_assets
            .iter()
            .find(|((p_id, c_id), _)| p_id == &plugin_id && c_id == &client_id)
            .and_then(|(_, (_running_plugin, _subscriptions, workers))| {
                workers.get(&format!("{}_worker", worker_name)).cloned()
            })
            .clone()
    }
    pub fn all_plugin_ids_for_plugin_location(
        &self,
        plugin_location: &RunPluginLocation,
        plugin_configuration: &PluginUserConfiguration,
    ) -> Result<Vec<PluginId>> {
        let err_context = || format!("Failed to get plugin ids for location {plugin_location}");
        let plugin_ids: Vec<PluginId> = self
            .plugin_assets
            .iter()
            .filter(|(_, (running_plugin, _subscriptions, _workers))| {
                let running_plugin = running_plugin.lock().unwrap();
                let env = running_plugin.env();
                let plugin_config = &env.plugin;
                let running_plugin_location = &plugin_config.location;
                let running_plugin_configuration = &plugin_config.initial_userspace_configuration;
                let location_matches = running_plugin_location == plugin_location
                    // A plugin declared `file:keys.lua` keeps `<cwd>/keys.lua` as its identity -
                    // that is what a dumped layout has to say - but its source was read from the
                    // user's plugin folder. A reload naming that file in full is the same plugin,
                    // and must reload it rather than be told it does not exist and started again
                    // in a second pane.
                    || match (plugin_location, env.resolved_source_path.as_ref()) {
                        (RunPluginLocation::File(requested), Some(resolved)) => requested == resolved,
                        _ => false,
                    };
                location_matches
                    && configuration_matches_for_reload(
                        running_plugin_configuration,
                        plugin_configuration,
                    )
            })
            .map(|((plugin_id, _client_id), _)| *plugin_id)
            .collect();
        if plugin_ids.is_empty() {
            return Err(ZellijError::PluginDoesNotExist).with_context(err_context);
        }
        Ok(plugin_ids)
    }
    pub fn clone_plugin_assets(
        &self,
    ) -> HashMap<RunPluginLocation, HashMap<PluginUserConfiguration, Vec<(PluginId, ClientId)>>>
    {
        let mut cloned_plugin_assets: HashMap<
            RunPluginLocation,
            HashMap<PluginUserConfiguration, Vec<(PluginId, ClientId)>>,
        > = HashMap::new();
        for ((plugin_id, client_id), (running_plugin, _, _)) in self.plugin_assets.iter() {
            let running_plugin = running_plugin.lock().unwrap();
            let plugin_config = &running_plugin.env().plugin;
            let running_plugin_location = &plugin_config.location;
            let running_plugin_configuration = &plugin_config.initial_userspace_configuration;
            match cloned_plugin_assets.get_mut(running_plugin_location) {
                Some(location_map) => match location_map.get_mut(running_plugin_configuration) {
                    Some(plugin_instances_info) => {
                        plugin_instances_info.push((*plugin_id, *client_id));
                    },
                    None => {
                        location_map.insert(
                            running_plugin_configuration.clone(),
                            vec![(*plugin_id, *client_id)],
                        );
                    },
                },
                None => {
                    let mut location_map = HashMap::new();
                    location_map.insert(
                        running_plugin_configuration.clone(),
                        vec![(*plugin_id, *client_id)],
                    );
                    cloned_plugin_assets.insert(running_plugin_location.clone(), location_map);
                },
            }
        }
        cloned_plugin_assets
    }
    pub fn all_plugin_ids(&self) -> Vec<(PluginId, ClientId)> {
        self.plugin_assets
            .iter()
            .map(|((plugin_id, client_id), _)| (*plugin_id, *client_id))
            .collect()
    }
    /// Whether this plugin already has an instance owned by this client.
    pub fn contains(&self, plugin_id: PluginId, client_id: ClientId) -> bool {
        self.plugin_assets.contains_key(&(plugin_id, client_id))
    }
    pub fn insert(
        &mut self,
        plugin_id: PluginId,
        client_id: ClientId,
        running_plugin: Arc<Mutex<RunningPlugin>>,
        subscriptions: Arc<Mutex<Subscriptions>>,
        running_workers: HashMap<String, UnboundedSender<MessageToWorker>>,
    ) {
        self.plugin_assets.insert(
            (plugin_id, client_id),
            (running_plugin, subscriptions, running_workers),
        );
    }
    /// Move a plugin's single instance from one owner client to another, eg. when the client
    /// owning a shared plugin disconnects. Returns true if an instance was moved.
    pub fn rekey_client(&mut self, plugin_id: PluginId, from: ClientId, to: ClientId) -> bool {
        if from == to {
            return false;
        }
        match self.plugin_assets.remove(&(plugin_id, from)) {
            Some(plugin_asset) => {
                {
                    let mut running_plugin = plugin_asset.0.lock().unwrap();
                    let plugin_env = running_plugin.env_mut();
                    plugin_env.owner_client_id = to;
                    plugin_env.client_id = to;
                }
                self.plugin_assets.insert((plugin_id, to), plugin_asset);
                true
            },
            None => false,
        }
    }
    pub fn owner_client_ids_of_plugin_id(&self, plugin_id: PluginId) -> Vec<ClientId> {
        self.plugin_assets
            .keys()
            .filter(|(p_id, _)| *p_id == plugin_id)
            .map(|(_, client_id)| *client_id)
            .collect()
    }
    pub fn run_plugin_of_plugin_id(&self, plugin_id: PluginId) -> Option<RunPlugin> {
        self.plugin_assets
            .iter()
            .find_map(|((p_id, _), (running_plugin, _, _))| {
                if *p_id == plugin_id {
                    let running_plugin = running_plugin.lock().unwrap();
                    let plugin_config = &running_plugin.env().plugin;
                    let run_plugin_location = plugin_config.location.clone();
                    let run_plugin_configuration =
                        plugin_config.initial_userspace_configuration.clone();
                    let initial_cwd = plugin_config.initial_cwd.clone();
                    Some(RunPlugin {
                        _allow_exec_host_cmd: false,
                        location: run_plugin_location,
                        configuration: run_plugin_configuration,
                        initial_cwd,
                        // Carried, so a plugin named by a relative `file:` URL can still be
                        // found in the user's plugin folder when this is used to start it
                        // again - resurrecting a session, say.
                        relative_path: plugin_config.relative_path.clone(),
                    })
                } else {
                    None
                }
            })
    }
    pub fn list_plugins(&self) -> BTreeMap<PluginId, RunPlugin> {
        let all_plugin_ids: HashSet<PluginId> = self
            .all_plugin_ids()
            .into_iter()
            .map(|(plugin_id, _client_id)| plugin_id)
            .collect();
        let mut plugin_ids_to_cmds: BTreeMap<PluginId, RunPlugin> = BTreeMap::new();
        for plugin_id in all_plugin_ids {
            let plugin_cmd = self.run_plugin_of_plugin_id(plugin_id);
            match plugin_cmd {
                Some(plugin_cmd) => {
                    plugin_ids_to_cmds.insert(plugin_id, plugin_cmd.clone());
                },
                None => log::error!("Plugin with id: {plugin_id} not found"),
            }
        }
        plugin_ids_to_cmds
    }
}

pub type Subscriptions = HashSet<EventType>;

/// Where a host command's reply goes.
///
/// A wasm plugin reads replies off its WASI stdin pipe: the host writes the response there
/// during the host call, and the guest reads it the moment the call returns. A Lua plugin has no
/// such pipe - it gets the value back as the return of the function it called - so its dispatcher
/// swaps in `Capture` for the duration of one command and takes the bytes afterwards.
///
/// Routing both through here is what lets all ~200 command handlers stay runtime-agnostic: they
/// call `wasi_write_object` exactly as before and never learn which runtime they are serving.
pub enum ResponseSink {
    /// Write into the plugin's WASI stdin pipe (wasm).
    WasiStdin,
    /// Collect the reply for the caller to take (Lua).
    Capture(Option<String>),
}

impl ResponseSink {
    /// Take whatever reply was captured, resetting the sink. Always `None` for `WasiStdin`,
    /// whose replies went to the pipe instead.
    pub fn take(&mut self) -> Option<String> {
        match self {
            ResponseSink::WasiStdin => None,
            ResponseSink::Capture(captured) => captured.take(),
        }
    }
}

/// The message a guest is stopped with when its cancel token is set.
pub const CANCELLED_BY_HOST: &str = "plugin cancelled by the host";

pub struct PluginEnv {
    pub plugin_id: PluginId,
    /// Set by the host to stop this instance: an unload, or session shutdown. Wasm checks it at
    /// every crossing of the host boundary (`Store::call_hook`), Lua every few thousand VM
    /// instructions, and either traps the running entrypoint with `CANCELLED_BY_HOST`. Shared by
    /// every instance of the plugin (one per client); see `WasmBridge::cancel_tokens` for why
    /// the host keeps its own handle rather than reaching in here.
    pub cancel: Arc<AtomicBool>,
    /// Set by whichever hook actually stopped this instance with `CANCELLED_BY_HOST`. Unlike
    /// `cancel`, which is shared and is set for idle instances too, this says the guest was
    /// interrupted mid-call — after which a wasm instance is not safe to call again (a trap does
    /// not unwind guest frames, so `zellij-tile`'s state cell stays borrowed and the next export
    /// panics), and a Lua one would only run away again. Per instance, unlike `cancel`.
    pub cancel_tripped: Arc<AtomicBool>,
    pub plugin: PluginConfig,
    pub permissions: Arc<Mutex<Option<HashSet<PermissionType>>>>,
    /// Whether this plugin has asked for permissions at all, set by `request_permission`.
    ///
    /// `permissions` alone cannot say: it is `None` both for a plugin whose request is still in
    /// flight and for one that never made a request, and those two have opposite fixes - wait
    /// for the answer, or ask in the first place. Shared by every instance of the plugin, like
    /// `permissions` itself.
    pub permissions_requested: Arc<AtomicBool>,
    pub senders: ThreadSenders,
    /// The WASI context backing a wasm plugin's filesystem and stdio. `None` for runtimes that
    /// have no WASI layer at all, such as Lua.
    pub wasi_ctx: Option<WasiCtx>,
    pub tab_index: Option<usize>,
    /// The client this instance is currently acting on behalf of. For a shared plugin (one
    /// instance for the whole session) this is set to the client whose event is being handled, so
    /// that host commands act on the right client. Use `owner_client_id` for anything keyed in
    /// the `PluginMap`.
    pub client_id: ClientId,
    /// The client this instance is keyed under in the `PluginMap`. Equal to `client_id` for
    /// non-shared plugins.
    pub owner_client_id: ClientId,
    #[allow(dead_code)]
    pub plugin_own_data_dir: PathBuf,
    /// The process-wide provider shared by recording and plugin data.
    pub data_access: Arc<DataAccess>,
    /// The work queues, shared with the plugin thread. A plugin enqueues and reads status
    /// straight from here rather than asking the plugin thread, because a plugin that waits for
    /// the plugin thread while holding its own lock deadlocks the moment another plugin loads
    /// (the loader lists every running plugin, which takes every plugin's lock).
    pub queues: Arc<Mutex<Queues>>,
    pub plugin_own_cache_dir: PathBuf,
    pub path_to_default_shell: PathBuf,
    pub default_shell: Option<TerminalAction>,
    pub layout_dir: Option<PathBuf>,
    /// Where the user's own plugins live. Read by `GetUserPluginDir`, and by the Lua
    /// loader as the folder to search when a plugin is named rather than pathed.
    pub user_plugin_dir: Option<PathBuf>,
    /// The file a Lua plugin's source was actually read from, which is not always
    /// `plugin.location`: a plugin declared `file:keys.lua` keeps `<cwd>/keys.lua` as its
    /// identity but is found in `user_plugin_dir`. A reload names the file in full, so this
    /// is what `all_plugin_ids_for_plugin_location` matches it against. `None` for wasm
    /// plugins, which are not reloadable by authoring and have their own lookup.
    pub resolved_source_path: Option<PathBuf>,
    pub plugin_cwd: PathBuf,
    pub session_env_vars: std::collections::BTreeMap<String, String>,
    /// The `plugins { }` alias table, shared with `WasmBridge` so it stays current across
    /// config reloads. Read only by `GetPluginAliasConfiguration`.
    pub plugin_aliases: Arc<Mutex<PluginAliases>>,
    pub input_pipes_to_unblock: Arc<Mutex<HashSet<String>>>,
    pub input_pipes_to_block: Arc<Mutex<HashSet<String>>>,
    pub default_mode: InputMode,
    pub subscriptions: Arc<Mutex<Subscriptions>>,
    pub stdin_pipe: Arc<Mutex<VecDeque<u8>>>,
    pub stdout_pipe: Arc<Mutex<VecDeque<u8>>>,
    pub keybinds: Keybinds,
    pub intercepting_key_presses: bool,
    pub store_limits: PluginResourceLimits,
    /// Where replies to host commands are delivered. See [`ResponseSink`].
    pub response_sink: ResponseSink,
}

#[derive(Clone)]
pub struct VecDequeInputStream(pub Arc<Mutex<VecDeque<u8>>>);

impl std::io::Read for VecDequeInputStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut inner = self.0.lock().unwrap();
        let len = std::cmp::min(buf.len(), inner.len());
        for (i, byte) in inner.drain(0..len).enumerate() {
            buf[i] = byte;
        }
        Ok(len)
    }
}

pub struct WriteOutputStream<T>(pub Arc<Mutex<T>>);

impl<T> Clone for WriteOutputStream<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T: Write + Send + 'static> std::io::Write for WriteOutputStream<T> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut inner = self.0.lock().unwrap();
        inner.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let mut inner = self.0.lock().unwrap();
        inner.flush()
    }
}

impl PluginEnv {
    // Get the name (path) of the containing plugin
    pub fn name(&self) -> String {
        format!("{} (ID {})", self.plugin.path.display(), self.plugin_id)
    }

    pub fn set_permissions(&mut self, permissions: HashSet<PermissionType>) {
        self.permissions.lock().unwrap().replace(permissions);
    }
}

#[derive(Eq, PartialEq, Hash)]
pub enum AtomicEvent {
    Resize,
}

/// The runtime actually executing a plugin. Zellij runs plugins compiled to wasm and plugins
/// written in Lua; everything above this enum - pane sizes, subscriptions, event routing,
/// rendering - is common to both, so the runtime difference is confined to here and to the
/// entrypoint methods on `RunningPlugin`.
///
/// This is an enum rather than a `Box<dyn Trait>` deliberately: there will only ever be a
/// couple of runtimes, and an enum keeps the wasm variant free of object-safety and `Send`
/// gymnastics.
pub enum PluginBackend {
    Wasm {
        store: Store<PluginEnv>,
        instance: Instance,
    },
    Lua(LuaPlugin),
}

pub struct RunningPlugin {
    backend: PluginBackend,
    pub rows: usize,
    pub columns: usize,
    next_event_ids: HashMap<AtomicEvent, usize>,
    last_applied_event_ids: HashMap<AtomicEvent, usize>,
    /// Event types this plugin has already been told it lacks the permission for. A denial is a
    /// standing misconfiguration, not an incident: subscribing to a high-frequency event without
    /// its permission would otherwise log once per keystroke and blow the log's 100KB cap in
    /// seconds. See `note_denied_event`.
    denied_events_logged: HashSet<EventType>,
}

impl RunningPlugin {
    pub fn new(store: Store<PluginEnv>, instance: Instance, rows: usize, columns: usize) -> Self {
        RunningPlugin {
            backend: PluginBackend::Wasm { store, instance },
            rows,
            columns,
            next_event_ids: HashMap::new(),
            last_applied_event_ids: HashMap::new(),
            denied_events_logged: HashSet::new(),
        }
    }
    /// The plugin's environment - the per-instance state every host command acts on. Reached
    /// through here rather than through the wasm `Store` so that call sites don't need to know
    /// which runtime is backing this plugin.
    pub fn new_lua(lua_plugin: LuaPlugin, rows: usize, columns: usize) -> Self {
        RunningPlugin {
            backend: PluginBackend::Lua(lua_plugin),
            rows,
            columns,
            next_event_ids: HashMap::new(),
            last_applied_event_ids: HashMap::new(),
            denied_events_logged: HashSet::new(),
        }
    }
    /// Record that an event of this type was denied to this plugin, returning whether it is the
    /// first of its type - which is the only one worth logging.
    pub fn note_denied_event(&mut self, event_type: EventType) -> bool {
        self.denied_events_logged.insert(event_type)
    }

    pub fn env(&self) -> &PluginEnv {
        match &self.backend {
            PluginBackend::Wasm { store, .. } => store.data(),
            PluginBackend::Lua(lua) => lua.env(),
        }
    }
    pub fn env_mut(&mut self) -> &mut PluginEnv {
        match &mut self.backend {
            PluginBackend::Wasm { store, .. } => store.data_mut(),
            PluginBackend::Lua(lua) => lua.env_mut(),
        }
    }

    /// Run the plugin's module-level initialization (wasm's `_start`). Called once, after the
    /// plugin is in the `PluginMap`, so that host commands it issues can find it.
    /// Run the plugin's module-level initialization (wasm's `_start`). Called once, after the
    /// plugin is in the `PluginMap`, so that host commands it issues can find it.
    pub fn start(&mut self) -> Result<()> {
        match &mut self.backend {
            PluginBackend::Wasm { store, instance } => {
                let start = instance
                    .get_typed_func::<(), ()>(&mut *store, "_start")
                    .map_err(|e| anyhow!(e))?;
                start.call(&mut *store, ()).map_err(|e| anyhow!(e))?;
                Ok(())
            },
            // A Lua plugin's source has already run by the time it gets here; running it is how
            // we obtained the table holding its entrypoints.
            PluginBackend::Lua(_) => Ok(()),
        }
    }

    /// Call the plugin's `load` entrypoint with its user configuration.
    ///
    /// Takes the `PluginUserConfiguration` rather than its inner map on purpose:
    /// `PluginUserConfiguration::new` strips reserved keys, so rebuilding one here would quietly
    /// drop configuration the plugin was meant to receive.
    pub fn load(&mut self, configuration: &PluginUserConfiguration) -> Result<()> {
        match &mut self.backend {
            PluginBackend::Wasm { store, instance } => {
                let protobuf_configuration: ProtobufPluginConfiguration = configuration
                    .clone()
                    .try_into()
                    .map_err(|e| anyhow!("failed to serialize user configuration: {:?}", e))?;
                let load = instance
                    .get_typed_func::<(), ()>(&mut *store, "load")
                    .map_err(|e| anyhow!(e))?;
                wasi_write_object(store.data_mut(), &protobuf_configuration.encode_to_vec())?;
                load.call(&mut *store, ()).map_err(|e| anyhow!(e))?;
                Ok(())
            },
            PluginBackend::Lua(lua) => lua.load(configuration.inner()),
        }
    }

    /// Call the plugin's `update` entrypoint, returning whether it asked to be re-rendered.
    ///
    /// An event the host cannot encode is deliberately non-fatal: it is logged and dropped
    /// rather than returned as an error, because the caller turns any error here into a plugin
    /// crash and the plugin is not the thing that failed.
    pub fn update(&mut self, event: &Event) -> Result<bool> {
        match &mut self.backend {
            PluginBackend::Wasm { store, instance } => {
                let protobuf_event: ProtobufEvent = match event.clone().try_into() {
                    Ok(protobuf_event) => protobuf_event,
                    Err(e) => {
                        // the host could not encode this event; the plugin itself is fine, so
                        // skip the event rather than kill it
                        Err::<(), _>(anyhow!(
                            "plugin {}: failed to serialize {}: {:?}",
                            self.env().plugin_id,
                            event,
                            e
                        ))
                        .non_fatal();
                        return Ok(false);
                    },
                };
                let update = instance
                    .get_typed_func::<(), i32>(&mut *store, "update")
                    .map_err(|e| anyhow!(e))?;
                wasi_write_object(store.data_mut(), &protobuf_event.encode_to_vec())?;
                let should_render = update.call(&mut *store, ()).map_err(|e| anyhow!(e))?;
                Ok(should_render == 1)
            },
            PluginBackend::Lua(lua) => lua.update(event),
        }
    }

    /// Call the plugin's `pipe` entrypoint, returning whether it asked to be re-rendered.
    /// `Ok(None)` means the plugin does not implement `pipe` at all, which is not an error.
    pub fn pipe(&mut self, message: &PipeMessage) -> Result<Option<bool>> {
        match &mut self.backend {
            PluginBackend::Wasm { store, instance } => {
                let Ok(pipe) = instance.get_typed_func::<(), i32>(&mut *store, "pipe") else {
                    return Ok(None);
                };
                let protobuf_message: ProtobufPipeMessage = message
                    .clone()
                    .try_into()
                    .map_err(|e| anyhow!("failed to serialize pipe message: {:?}", e))?;
                wasi_write_object(store.data_mut(), &protobuf_message.encode_to_vec())?;
                let should_render = pipe.call(&mut *store, ()).map_err(|e| anyhow!(e))?;
                Ok(Some(should_render == 1))
            },
            PluginBackend::Lua(lua) => lua.pipe(message).map(Some),
        }
    }

    /// Call the plugin's `render` entrypoint and collect the frame it produced. The bytes are a
    /// full-screen ANSI frame - the pane replaces its whole viewport with them.
    pub fn render(&mut self, rows: usize, columns: usize) -> Result<Vec<u8>> {
        match &mut self.backend {
            PluginBackend::Wasm { store, instance } => {
                let render = instance
                    .get_typed_func::<(i32, i32), ()>(&mut *store, "render")
                    .map_err(|e| anyhow!(e))?;
                render
                    .call(&mut *store, (rows as i32, columns as i32))
                    .map_err(|e| anyhow!(e))?;
                Ok(wasi_read_string(store.data())?.into_bytes())
            },
            PluginBackend::Lua(lua) => lua.render(rows, columns),
        }
    }

    pub fn next_event_id(&mut self, atomic_event: AtomicEvent) -> usize {
        let current_event_id = *self.next_event_ids.get(&atomic_event).unwrap_or(&0);
        if current_event_id < usize::MAX {
            let next_event_id = current_event_id + 1;
            self.next_event_ids.insert(atomic_event, next_event_id);
            current_event_id
        } else {
            let current_event_id = 0;
            let next_event_id = 1;
            self.last_applied_event_ids.remove(&atomic_event);
            self.next_event_ids.insert(atomic_event, next_event_id);
            current_event_id
        }
    }
    pub fn apply_event_id(&mut self, atomic_event: AtomicEvent, event_id: usize) -> bool {
        if &event_id >= self.last_applied_event_ids.get(&atomic_event).unwrap_or(&0) {
            self.last_applied_event_ids.insert(atomic_event, event_id);
            true
        } else {
            false
        }
    }
    pub fn update_keybinds(&mut self, keybinds: Keybinds) {
        self.env_mut().keybinds = keybinds;
    }
    pub fn update_default_mode(&mut self, default_mode: InputMode) {
        self.env_mut().default_mode = default_mode;
    }
    pub fn update_default_shell(&mut self, default_shell: Option<TerminalAction>) {
        self.env_mut().default_shell = default_shell;
    }
    pub fn update_layout_dir(&mut self, layout_dir: Option<PathBuf>) {
        self.env_mut().layout_dir = layout_dir;
    }
    pub fn update_user_plugin_dir(&mut self, user_plugin_dir: Option<PathBuf>) {
        self.env_mut().user_plugin_dir = user_plugin_dir;
    }
}

#[cfg(test)]
mod tests {
    use super::configuration_matches_for_reload;
    use zellij_utils::input::layout::PluginUserConfiguration;

    fn configuration(entries: &[(&str, &str)]) -> PluginUserConfiguration {
        PluginUserConfiguration::new(
            entries
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    #[test]
    fn reload_lookup_matches_a_configured_plugin() {
        // eg. hocus launched from an alias, then reloaded by its bare location
        let running = configuration(&[
            ("dobbs_path", "/home/bob/dobbs"),
            ("shared", "true"),
            ("caller_cwd", "/home/bob"),
            ("caller_terminal_pane_id", "7"),
        ]);

        // no configuration at all: "reload whatever runs at this location"
        assert!(configuration_matches_for_reload(
            &running,
            &configuration(&[])
        ));
        // only caller_cwd differs, which is per-launch and not part of the identity
        assert!(configuration_matches_for_reload(
            &running,
            &configuration(&[
                ("dobbs_path", "/home/bob/dobbs"),
                ("shared", "true"),
                ("caller_cwd", "/tmp"),
                ("caller_terminal_pane_id", "9"),
            ])
        ));
        // an explicit, differing configuration is still a different plugin
        assert!(!configuration_matches_for_reload(
            &running,
            &configuration(&[("dobbs_path", "/other/dobbs"), ("shared", "true")])
        ));
    }
}
