use crate::steel::SteelPlugin;
use crate::PluginId;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, PoisonError, Weak,
    },
};
use yinz_types::internal_ids::TabId;

use crate::ClientId;
use yinz_bus::plugins::{PluginInstanceId, PluginInstruction};
use yinz_bus::thread_bus::ThreadSenders;

use yinz_storage::data_access::DataAccess;

use yinz_runtime::errors::prelude::*;
use {
    yinz_config::plugins::PluginConfig, yinz_types::data::EventType, yinz_types::data::InputMode,
    yinz_types::input::command::TerminalAction, yinz_types::input::keybinds::Keybinds,
    yinz_types::input::layout::PluginUserConfiguration, yinz_types::input::layout::RunPlugin,
    yinz_types::input::layout::RunPluginLocation, yinz_types::input::plugins::PluginAliases,
};
use {yinz_types::data::Event, yinz_types::data::PermissionType, yinz_types::data::PipeMessage};

// Guest execution and disposal happen outside the map lock, on each plugin's pinned executor.
#[derive(Default)]
pub struct PluginMap {
    // Value-only retry state survives failed instances without retaining their resources.
    plugin_metadata: BTreeMap<PluginId, PluginMetadata>,
    load_tokens: HashMap<PluginId, Arc<AtomicBool>>,
    // A pending reload keeps the owner's /data alive across superseded cleanup jobs.
    pub(crate) reload_data_dirs: HashMap<PluginId, Option<Arc<yinz_runtime::owned_dir::OwnedDir>>>,
    pub(crate) configuration_version: u64,
    pub(crate) client_configurations: HashMap<ClientId, (u64, PluginConfiguration)>,
    pub(crate) connected_clients: Arc<Mutex<Vec<ClientId>>>,
    pending_configuration_clients: HashMap<ClientId, usize>,
    plugin_assets: HashMap<(PluginId, ClientId), PluginAssets>,
    instance_ids: HashMap<(PluginId, ClientId), PluginInstanceId>,
    instance_cwds: HashMap<(PluginId, ClientId), PathBuf>,
    availability: HashMap<(PluginId, ClientId), Arc<AtomicBool>>,
    /// Each instance's [`ResizeSlot`], held here too so a resize can be asked of an instance
    /// without waiting for its guest lock.
    resize_slots: HashMap<(PluginId, ClientId), Arc<ResizeSlot>>,
    client_cancellations: HashMap<(PluginId, ClientId), (Option<u64>, Arc<AtomicBool>)>,
    workers: HashMap<(PluginId, ClientId), Arc<crate::background_worker::Workers>>,
}

/// Cloned loading contexts share one registration until their last loader is dropped.
#[derive(Debug)]
pub(crate) struct PendingConfiguration {
    map: Weak<Mutex<PluginMap>>,
    client: ClientId,
}

impl Drop for PendingConfiguration {
    fn drop(&mut self) {
        if let Some(map) = self.map.upgrade() {
            let mut map = map.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(count) = map.pending_configuration_clients.get_mut(&self.client) {
                *count -= 1;
                if *count == 0 {
                    map.pending_configuration_clients.remove(&self.client);
                }
            }
            map.prune_client_configuration(self.client);
        }
    }
}

#[derive(Clone)]
pub(crate) struct PluginConfiguration {
    pub keybinds: Keybinds,
    pub default_mode: InputMode,
    pub default_shell: Option<TerminalAction>,
    pub layout_dir: Option<PathBuf>,
    pub user_plugin_dir: Option<PathBuf>,
}

impl PluginConfiguration {
    pub fn apply_to(&self, plugin: &mut RunningPlugin) {
        plugin.update_keybinds(self.keybinds.clone());
        plugin.update_default_mode(self.default_mode);
        plugin.update_default_shell(self.default_shell.clone());
        plugin.update_layout_dir(self.layout_dir.clone());
        plugin.update_user_plugin_dir(self.user_plugin_dir.clone());
    }
}

#[derive(Clone)]
pub(crate) struct PluginMetadata {
    pub config: PluginConfig,
    pub data_owner: url::Url,
    pub resolved_source_path: Option<PathBuf>,
    pub cache_dir: PathBuf,
    pub cwd: PathBuf,
    pub tab_index: Option<TabId>,
    pub size: (usize, usize),
    pub owner_client_id: ClientId,
}

impl PluginMetadata {
    fn from_running(plugin: &RunningPlugin) -> Self {
        let env = plugin.env();
        Self {
            config: env.plugin_config.clone(),
            data_owner: env.data_owner.clone(),
            resolved_source_path: env.resolved_source_path.clone(),
            cache_dir: env.plugin_own_cache_dir.clone(),
            cwd: env.plugin_cwd.clone(),
            tab_index: env.tab_index,
            // The newest size asked, not the last drawn: a resize still owed a render when the
            // instance goes is the size a reload should start at.
            size: plugin.resize.wanted(),
            owner_client_id: env.owner_client_id,
        }
    }

    fn run_plugin(&self) -> RunPlugin {
        RunPlugin {
            _allow_exec_host_cmd: false,
            location: self.config.location.clone(),
            configuration: self.config.initial_userspace_configuration.clone(),
            initial_cwd: self.config.initial_cwd.clone(),
            relative_path: self.config.relative_path.clone(),
            declared_path: matches!(self.config.location, RunPluginLocation::File(_))
                .then(|| self.config.path.clone()),
        }
    }
}

pub type PluginAssets = (Arc<Mutex<RunningPlugin>>, Arc<Mutex<Subscriptions>>);

/// A running plugin instance and the events it subscribes to, each behind its own lock so
/// that updating one instance never blocks rendering another.
pub type RunningPluginAndSubscriptions = (Arc<Mutex<RunningPlugin>>, Arc<Mutex<Subscriptions>>);

/// A `RunningPluginAndSubscriptions` together with the plugin and client it was loaded for.
pub type RunningPluginEntry = (
    PluginId,
    ClientId,
    Arc<Mutex<RunningPlugin>>,
    Arc<Mutex<Subscriptions>>,
);

/// Configuration match used when looking up running plugins to reload.
///
/// Deliberately looser than plain equality: caller context is injected into an alias's
/// configuration per-launch (`Pty::spawn_plugin`), so it is never part of a plugin's identity.
/// `RunPluginOrAlias::is_equivalent_to_run` ignores it for the same reason. And an empty request
/// (eg. `yinz action start-or-reload-plugin yinz:agent`, which carries none of the alias'
/// configuration) means "whatever is running at this location", otherwise reloading a
/// configured plugin by its bare location would start a second copy of it instead.
fn configuration_matches_for_reload(
    running: &PluginUserConfiguration,
    requested: &PluginUserConfiguration,
) -> bool {
    let requested = requested.without_caller_context();
    requested.is_empty() || running.without_caller_context() == requested
}

impl PluginMap {
    pub(crate) fn pending_configuration(
        &mut self,
        map: &Arc<Mutex<Self>>,
        client: ClientId,
    ) -> Arc<PendingConfiguration> {
        *self
            .pending_configuration_clients
            .entry(client)
            .or_default() += 1;
        Arc::new(PendingConfiguration {
            map: Arc::downgrade(map),
            client,
        })
    }

    pub(crate) fn needs_client_configuration(&self, client: ClientId) -> bool {
        // Connected clients may still be cloned by another client's loader.
        self.connected_clients.lock().unwrap().contains(&client)
            || self.pending_configuration_clients.contains_key(&client)
            || self.availability.iter().any(|((_, owner), unavailable)| {
                *owner == client && !unavailable.load(Ordering::Acquire)
            })
    }

    pub(crate) fn prune_client_configuration(&mut self, client: ClientId) {
        if !self.needs_client_configuration(client) {
            self.client_configurations.remove(&client);
        }
    }

    pub(crate) fn set_load_token(&mut self, plugin: PluginId, cancel: Arc<AtomicBool>) {
        self.load_tokens.insert(plugin, cancel);
    }

    pub(crate) fn load_is_current(&self, plugin: PluginId, cancel: &Arc<AtomicBool>) -> bool {
        !cancel.load(Ordering::Acquire)
            && self
                .load_tokens
                .get(&plugin)
                .is_some_and(|current| Arc::ptr_eq(current, cancel))
    }

    pub(crate) fn instance_cwd(
        &self,
        plugin: PluginId,
        client: Option<ClientId>,
    ) -> Option<PathBuf> {
        match client {
            Some(client) => self.instance_cwds.get(&(plugin, client)),
            None => self
                .instance_cwds
                .iter()
                .find_map(|((id, _), cwd)| (*id == plugin).then_some(cwd)),
        }
        .cloned()
    }

    pub(crate) fn set_instance_cwd(&mut self, plugin: PluginId, client: ClientId, cwd: PathBuf) {
        if let Some(current) = self.instance_cwds.get_mut(&(plugin, client)) {
            *current = cwd;
        }
    }

    pub(crate) fn permission_instance(
        &self,
        plugin: PluginId,
        instance: PluginInstanceId,
    ) -> Option<(ClientId, Arc<Mutex<RunningPlugin>>)> {
        self.instance_ids.iter().find_map(|(key, id)| {
            (key.0 == plugin && *id == instance && !self.availability[key].load(Ordering::Acquire))
                .then(|| (key.1, self.plugin_assets[key].0.clone()))
        })
    }

    pub fn pipe_instances(&mut self) -> Vec<(RunningPluginEntry, PluginInstanceId)> {
        self.running_plugins_and_subscriptions()
            .into_iter()
            .filter_map(|entry| {
                let key = (entry.0, entry.1);
                let id = *self.instance_ids.get(&key)?;
                (self.contains(key.0, key.1) && !self.availability[&key].load(Ordering::Acquire))
                    .then_some((entry, id))
            })
            .collect()
    }

    pub fn pipe_failure_instances(
        &mut self,
        plugin: PluginId,
        instance: Option<PluginInstanceId>,
        generation: Option<u64>,
    ) -> Vec<(PluginInstanceId, Arc<Mutex<RunningPlugin>>)> {
        self.instance_ids
            .iter()
            .filter_map(|(key, id)| {
                if key.0 != plugin || instance.is_some_and(|expected| expected != *id) {
                    return None;
                }
                if let Some(generation) = generation
                    && !self
                        .client_cancellations
                        .get(key)
                        .is_some_and(|(current, cancel)| {
                            *current == Some(generation) && !cancel.load(Ordering::Acquire)
                        })
                {
                    return None;
                }
                self.availability[key].store(true, Ordering::Release);
                Some((*id, self.plugin_assets[key].0.clone()))
            })
            .collect()
    }

    pub(crate) fn remember_plugin(&mut self, id: PluginId, metadata: PluginMetadata) {
        self.plugin_metadata.insert(id, metadata);
    }
    pub(crate) fn cached_metadata(&self, id: PluginId) -> Option<&PluginMetadata> {
        self.plugin_metadata.get(&id)
    }
    pub fn retire_instances(&mut self, plugin: PluginId) {
        for ((id, _), unavailable) in &self.availability {
            if *id == plugin {
                unavailable.store(true, Ordering::Release);
            }
        }
        self.instance_ids.retain(|(id, _), _| *id != plugin);
    }
    pub fn remove_plugins(&mut self, pid: PluginId) -> HashMap<(PluginId, ClientId), PluginAssets> {
        self.retire_instances(pid);
        self.load_tokens.remove(&pid);
        self.reload_data_dirs.remove(&pid);
        self.instance_cwds.retain(|(id, _), _| *id != pid);
        self.availability.retain(|(id, _), _| *id != pid);
        self.resize_slots.retain(|(id, _), _| *id != pid);
        self.plugin_metadata.remove(&pid);
        self.workers.retain(|(id, _), _| *id != pid);
        self.client_cancellations.retain(|(id, _), _| *id != pid);
        let mut removed = HashMap::new();
        let ids_in_plugin_map: Vec<(PluginId, ClientId)> =
            self.plugin_assets.keys().copied().collect();
        for (plugin_id, client_id) in ids_in_plugin_map {
            if pid == plugin_id
                && let Some(plugin_asset) = self.plugin_assets.remove(&(plugin_id, client_id))
            {
                removed.insert((plugin_id, client_id), plugin_asset);
            }
        }
        for (_, client) in removed.keys() {
            self.prune_client_configuration(*client);
        }
        removed
    }
    pub fn remove_instance(
        &mut self,
        plugin_id: PluginId,
        client_id: ClientId,
    ) -> Option<PluginAssets> {
        self.instance_ids.remove(&(plugin_id, client_id));
        self.instance_cwds.remove(&(plugin_id, client_id));
        if let Some(unavailable) = self.availability.remove(&(plugin_id, client_id)) {
            unavailable.store(true, Ordering::Release);
        }
        self.resize_slots.remove(&(plugin_id, client_id));
        self.workers.remove(&(plugin_id, client_id));
        self.client_cancellations.remove(&(plugin_id, client_id));
        let assets = self.plugin_assets.remove(&(plugin_id, client_id))?;
        self.plugin_metadata.insert(
            plugin_id,
            PluginMetadata::from_running(&assets.0.lock().unwrap()),
        );
        self.prune_client_configuration(client_id);
        Some(assets)
    }

    pub(crate) fn remove_matching_instance(
        &mut self,
        plugin_id: PluginId,
        instance: &Arc<Mutex<RunningPlugin>>,
    ) -> Option<PluginAssets> {
        // Cleanup must preserve replacements and follow shared instances across map rekeys.
        let client = self
            .plugin_assets
            .iter()
            .find_map(|((id, client), (current, _))| {
                (*id == plugin_id && Arc::ptr_eq(current, instance)).then_some(*client)
            })?;
        self.remove_instance(plugin_id, client)
    }

    /// Detached instances release their resources while retaining retry metadata.
    pub fn remove_disconnected_instances(
        &mut self,
        plugin_id: PluginId,
        connected: &[ClientId],
    ) -> Vec<(ClientId, PluginAssets)> {
        let owners = self.owner_client_ids_of_plugin_id(plugin_id);
        owners
            .into_iter()
            .filter(|owner| {
                !connected.contains(owner)
                    || self
                        .client_cancellations
                        .get(&(plugin_id, *owner))
                        .is_some_and(|(_, cancel)| cancel.load(Ordering::Acquire))
            })
            .collect::<Vec<_>>()
            .into_iter()
            .filter_map(|owner| {
                self.remove_instance(plugin_id, owner)
                    .map(|assets| (owner, assets))
            })
            .collect()
    }

    pub fn plugin_ids(&self) -> Vec<PluginId> {
        self.plugin_metadata.keys().copied().collect()
    }
    pub fn running_plugins(&mut self) -> Vec<(PluginId, ClientId, Arc<Mutex<RunningPlugin>>)> {
        self.plugin_assets
            .iter()
            .map(|((plugin_id, client_id), (running_plugin, _))| {
                (*plugin_id, *client_id, running_plugin.clone())
            })
            .collect()
    }
    /// Every instance of `plugin_id`, with the [`ResizeSlot`] a resize is asked of it through.
    pub fn resize_targets(
        &self,
        plugin_id: PluginId,
    ) -> Vec<(ClientId, Arc<Mutex<RunningPlugin>>, Arc<ResizeSlot>)> {
        self.plugin_assets
            .iter()
            .filter(|((id, _), _)| *id == plugin_id)
            .filter_map(|(key, (running_plugin, _))| {
                let slot = self.resize_slots.get(key)?;
                Some((key.1, running_plugin.clone(), slot.clone()))
            })
            .collect()
    }
    #[cfg(test)]
    pub fn has_resize_slots(&self, plugin_id: PluginId) -> bool {
        self.resize_slots.keys().any(|(id, _)| *id == plugin_id)
    }
    pub fn running_plugins_and_subscriptions(&mut self) -> Vec<RunningPluginEntry> {
        self.plugin_assets
            .iter()
            .map(
                |((plugin_id, client_id), (running_plugin, subscriptions))| {
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
    ) -> Option<RunningPluginAndSubscriptions> {
        self.plugin_assets
            .get(&(plugin_id, client_id))
            .map(|(running_plugin, subscriptions)| (running_plugin.clone(), subscriptions.clone()))
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
                .map(|(running_plugin, _)| running_plugin.clone()),
            None => self
                .plugin_assets
                .iter()
                .find(|((p_id, _), _)| *p_id == plugin_id)
                .map(|(_, (running_plugin, _))| running_plugin.clone()),
        }
    }

    pub fn all_plugin_ids_for_plugin_location(
        &self,
        plugin_location: &RunPluginLocation,
        plugin_configuration: &PluginUserConfiguration,
        normalize: impl Fn(&RunPluginLocation) -> RunPluginLocation,
    ) -> Result<Vec<PluginId>> {
        let err_context = || format!("Failed to get plugin ids for location {plugin_location}");
        let plugin_ids: Vec<PluginId> = self
            .plugin_metadata
            .iter()
            .filter(|(_, metadata)| {
                let plugin_config = &metadata.config;
                let running_plugin_location = &plugin_config.location;
                let running_plugin_configuration = &plugin_config.initial_userspace_configuration;
                let loaded_location =
                    match (running_plugin_location, &metadata.resolved_source_path) {
                        (RunPluginLocation::File(_), Some(path)) => {
                            RunPluginLocation::File(path.clone())
                        },
                        _ => normalize(running_plugin_location),
                    };
                let location_matches = running_plugin_location == plugin_location
                    // Match plugin-directory fallback and the declared path's current target,
                    // which can change when a symlink is retargeted after loading.
                    || match (normalize(plugin_location), loaded_location) {
                        (RunPluginLocation::File(requested), RunPluginLocation::File(resolved)) => {
                            let requested = requested.canonicalize().unwrap_or(requested);
                            let declared = matches!(running_plugin_location, RunPluginLocation::File(_))
                                .then_some(&plugin_config.path);
                            std::iter::once(&resolved).chain(declared)
                                .any(|path| requested == path.canonicalize().unwrap_or_else(|_| path.clone()))
                        },
                        (requested, resolved) => requested == resolved,
                    };
                location_matches
                    && configuration_matches_for_reload(
                        running_plugin_configuration,
                        plugin_configuration,
                    )
            })
            .map(|(plugin_id, _)| *plugin_id)
            .collect();
        if plugin_ids.is_empty() {
            return Err(YinzError::PluginDoesNotExist).with_context(err_context);
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
        for (plugin_id, metadata) in &self.plugin_metadata {
            let mut owners = self.owner_client_ids_of_plugin_id(*plugin_id);
            if owners.is_empty() {
                owners.push(metadata.owner_client_id);
            }
            cloned_plugin_assets
                .entry(metadata.config.location.clone())
                .or_default()
                .entry(metadata.config.initial_userspace_configuration.clone())
                .or_default()
                .extend(owners.into_iter().map(|client| (*plugin_id, client)));
        }
        cloned_plugin_assets
    }
    #[cfg(test)]
    pub fn all_plugin_ids(&self) -> Vec<(PluginId, ClientId)> {
        self.plugin_assets
            .iter()
            .map(|((plugin_id, client_id), _)| (*plugin_id, *client_id))
            .collect()
    }
    /// Whether this plugin already has an instance owned by this client.
    pub fn contains(&self, plugin_id: PluginId, client_id: ClientId) -> bool {
        self.plugin_assets.contains_key(&(plugin_id, client_id))
            && !self
                .client_cancellations
                .get(&(plugin_id, client_id))
                .is_some_and(|(_, cancel)| cancel.load(Ordering::Acquire))
    }

    pub(crate) fn has_available_instance(&self, plugin_id: PluginId) -> bool {
        self.availability.iter().any(|((id, client), unavailable)| {
            *id == plugin_id && self.contains(*id, *client) && !unavailable.load(Ordering::Acquire)
        })
    }
    /// Validate failure identity without locking an isolate that may be executing guest code.
    pub fn contains_generation(&self, plugin_id: PluginId, generation: u64) -> bool {
        self.client_cancellations
            .iter()
            .any(|((id, _), (current, cancel))| {
                *id == plugin_id && *current == Some(generation) && !cancel.load(Ordering::Acquire)
            })
    }
    pub(crate) fn worker_routes(
        &self,
        plugin_id: PluginId,
        generation: u64,
    ) -> Option<Arc<crate::background_worker::Workers>> {
        self.workers.iter().find_map(|((id, client), workers)| {
            (*id == plugin_id
                && self.contains(plugin_id, *client)
                && self.contains_generation(plugin_id, workers.parent_generation)
                && workers.contains(generation))
            .then(|| workers.clone())
        })
    }
    /// A queued reload supersedes old failure reports before its replacement is instantiated.
    pub fn invalidate_generations(&mut self, plugin_id: PluginId) {
        for ((id, _), (generation, _)) in &mut self.client_cancellations {
            if *id == plugin_id {
                *generation = None;
            }
        }
    }
    pub fn insert(
        &mut self,
        plugin_id: PluginId,
        client_id: ClientId,
        running_plugin: Arc<Mutex<RunningPlugin>>,
        subscriptions: Arc<Mutex<Subscriptions>>,
    ) -> Option<PluginAssets> {
        {
            // A poisoned guest still holds the identity and cwd needed to file it.
            let plugin = running_plugin
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            self.availability
                .insert((plugin_id, client_id), plugin.env().unavailable.clone());
            self.instance_ids
                .insert((plugin_id, client_id), plugin.instance_id);
            self.resize_slots
                .insert((plugin_id, client_id), plugin.resize.clone());
            self.instance_cwds
                .insert((plugin_id, client_id), plugin.env().plugin_cwd.clone());
            self.plugin_metadata
                .insert(plugin_id, PluginMetadata::from_running(&plugin));
            if let Some(workers) = &plugin.runtime.env().workers {
                self.workers.insert((plugin_id, client_id), workers.clone());
            }
            self.client_cancellations.insert(
                (plugin_id, client_id),
                (
                    Some(plugin.runtime.generation()),
                    plugin.runtime.client_cancel(),
                ),
            );
        }
        // The caller disposes any previous instance after releasing the map lock.
        self.plugin_assets
            .insert((plugin_id, client_id), (running_plugin, subscriptions))
    }
    /// Move a plugin's single instance from one owner client to another, eg. when the client
    /// owning a shared plugin disconnects. Returns true if an instance was moved.
    pub fn rekey_client(&mut self, plugin_id: PluginId, from: ClientId, to: ClientId) -> bool {
        if from == to {
            return false;
        }
        match self.plugin_assets.remove(&(plugin_id, from)) {
            Some(plugin_asset) => {
                if let Some(unavailable) = self.availability.remove(&(plugin_id, from)) {
                    self.availability.insert((plugin_id, to), unavailable);
                }
                if let Some(id) = self.instance_ids.remove(&(plugin_id, from)) {
                    self.instance_ids.insert((plugin_id, to), id);
                }
                if let Some(cwd) = self.instance_cwds.remove(&(plugin_id, from)) {
                    self.instance_cwds.insert((plugin_id, to), cwd);
                }
                if let Some(slot) = self.resize_slots.remove(&(plugin_id, from)) {
                    self.resize_slots.insert((plugin_id, to), slot);
                }
                if let Some(workers) = self.workers.remove(&(plugin_id, from)) {
                    self.workers.insert((plugin_id, to), workers);
                }
                if let Some(metadata) = self.plugin_metadata.get_mut(&plugin_id) {
                    metadata.owner_client_id = to;
                }
                if let Some(cancel) = self.client_cancellations.remove(&(plugin_id, from)) {
                    self.client_cancellations.insert((plugin_id, to), cancel);
                }
                {
                    let mut running_plugin = plugin_asset.0.lock().unwrap();
                    let plugin_env = running_plugin.env_mut();
                    plugin_env.owner_client_id = to;
                    plugin_env.client_id = to;
                    if plugin_env.tab_index.is_none() {
                        // The screen routes background pane/tab updates by owner too.
                        // Move that registration with the instance, or a reconnect leaves
                        // agent-service unaware of new panes and their checkout titles.
                        for (client_id, events) in [
                            (from, HashSet::new()),
                            (
                                to,
                                plugin_env
                                    .subscriptions
                                    .lock()
                                    .unwrap_or_else(PoisonError::into_inner)
                                    .clone(),
                            ),
                        ] {
                            plugin_env
                                .senders
                                .send_to_plugin(PluginInstruction::PluginSubscribedToEvents(
                                    plugin_id,
                                    client_id,
                                    events.clone(),
                                    Some(events),
                                ))
                                .context("failed to move background plugin subscriptions")
                                .non_fatal();
                        }
                    }
                }
                self.plugin_assets.insert((plugin_id, to), plugin_asset);
                self.prune_client_configuration(from);
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
        self.plugin_metadata
            .get(&plugin_id)
            .map(PluginMetadata::run_plugin)
    }
    /// Refresh the declared file identity after a successful reload.
    pub(crate) fn update_reloaded_file_location(
        &mut self,
        plugin_id: PluginId,
    ) -> Option<RunPluginLocation> {
        let metadata = self.plugin_metadata.get_mut(&plugin_id)?;
        if !matches!(metadata.config.location, RunPluginLocation::File(_)) {
            return None;
        }
        // A relative declaration may load from user_plugin_dir while its declared path is
        // absent. Keep that identity so the next relative launch still finds this pane.
        let path = &metadata.config.path;
        let location =
            RunPluginLocation::File(path.canonicalize().unwrap_or_else(|_| path.clone()));
        metadata.config.location = location.clone();
        for ((id, _), (plugin, _)) in &self.plugin_assets {
            if *id == plugin_id {
                plugin.lock().unwrap().env_mut().plugin_config.location = location.clone();
            }
        }
        Some(location)
    }

    pub(crate) fn cache_dir_of_plugin_id(&self, plugin_id: PluginId) -> Option<PathBuf> {
        self.plugin_metadata
            .get(&plugin_id)
            .map(|metadata| metadata.cache_dir.clone())
    }

    pub fn plugin_config_of_plugin_id(&self, plugin_id: PluginId) -> Option<PluginConfig> {
        self.plugin_metadata
            .get(&plugin_id)
            .map(|metadata| metadata.config.clone())
    }
    pub fn list_plugins(&self) -> BTreeMap<PluginId, RunPlugin> {
        self.plugin_metadata
            .iter()
            .map(|(id, metadata)| (*id, metadata.run_plugin()))
            .collect()
    }
    pub(crate) fn log_identity(&self, plugin_id: PluginId) -> Option<String> {
        self.plugin_metadata.get(&plugin_id).map(|metadata| {
            crate::plugin_authoring::LogIdentity::new(
                &metadata.config.location,
                metadata
                    .resolved_source_path
                    .as_deref()
                    .unwrap_or(&metadata.config.path),
                &metadata.cwd,
            )
            .marker()
        })
    }

    pub(crate) fn reload_metadata(&self, plugin_id: PluginId) -> Option<PluginMetadata> {
        let mut metadata = self.plugin_metadata.get(&plugin_id)?.clone();
        if let Some((key, (plugin, _))) = self
            .plugin_assets
            .iter()
            .find(|((id, _), _)| *id == plugin_id)
        {
            if let Ok(plugin) = plugin.try_lock() {
                return Some(PluginMetadata::from_running(&plugin));
            }
            // Attachment and reload must not wait for a guest callback to finish.
            metadata.owner_client_id = key.1;
            if let Some(size) = self.resize_slots.get(key) {
                metadata.size = size.wanted();
            }
            if let Some(cwd) = self.instance_cwds.get(key) {
                metadata.cwd = cwd.clone();
            }
        }
        Some(metadata)
    }
}

pub type Subscriptions = HashSet<EventType>;

/// The encoded reply to one synchronous host command.
pub type ResponseSink = Option<Vec<u8>>;

/// The host cancelled the instance or its active call.
pub const CANCELLED_BY_HOST: &str = "plugin cancelled by the host";

/// One registry per bridge/data provider; lock before registering, refreshing or removing watches.
pub type WatchGenerations = Arc<Mutex<HashMap<yinz_types::internal_ids::DataWatchId, u64>>>;

pub type PluginRoles = Arc<Mutex<HashMap<PluginId, bool>>>;

pub struct PluginEnv {
    pub yinz_data_dir: PathBuf,
    pub plugin_roles: PluginRoles,
    pub background: bool,
    pub pick_request: Option<yinz_types::pick::PickRequest>,
    pub pick_cancelled: Option<Arc<AtomicBool>>,
    pub pick_opening: Option<Arc<AtomicBool>>,
    pub plugin_id: PluginId,
    pub instance_id: PluginInstanceId,
    pub unavailable: Arc<AtomicBool>,
    /// Cancels every instance of this plugin on unload or session shutdown.
    /// Each runtime watchdog kills an active worker when this token is set.
    pub cancel: Arc<AtomicBool>,
    /// Records interruption during an active call, which makes the worker unusable.
    /// Unlike `cancel`, this flag belongs to one instance and excludes idle cancellation.
    pub cancel_tripped: Arc<AtomicBool>,
    /// Identifies the runtime instance that emitted a failure, including host ReportPanic.
    pub generation: Option<u64>,
    /// Preparations owned by this generation, retaining the original acting client.
    pub(crate) session_preparations: HashSet<(ClientId, String)>,
    /// Named workers share the owning instance's permissions and lifetime.
    pub(crate) workers: Option<Arc<crate::background_worker::Workers>>,
    pub plugin_config: PluginConfig,
    /// Original storage identity, unaffected by source changes on reload.
    pub data_owner: url::Url,
    pub permissions: Arc<Mutex<Option<HashSet<PermissionType>>>>,
    /// Whether this plugin has asked for permissions at all, set by `request_permission`.
    ///
    /// `permissions` alone cannot say: it is `None` both for a plugin whose request is still in
    /// flight and for one that never made a request, and those two have opposite fixes - wait
    /// for the answer, or ask in the first place. Shared by every instance of the plugin, like
    /// `permissions` itself.
    pub permissions_requested: Arc<AtomicBool>,
    pub senders: ThreadSenders,
    pub tab_index: Option<TabId>,
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
    /// Production environments own /data; standalone interpreter tests supply their own fixtures.
    pub temporary_data_dir: Option<Arc<yinz_runtime::owned_dir::OwnedDir>>,
    /// The process-wide provider for plugin data.
    pub data_access: Arc<DataAccess>,
    pub watch_generations: WatchGenerations,
    pub plugin_own_cache_dir: PathBuf,
    pub path_to_default_shell: PathBuf,
    pub default_shell: Option<TerminalAction>,
    pub layout_dir: Option<PathBuf>,
    /// Where the user's own plugins live. Read by `GetUserPluginDir`, and by the
    /// loader as the folder to search when a plugin is named rather than pathed.
    pub user_plugin_dir: Option<PathBuf>,
    /// The canonical source file selected by the loader.
    /// This can differ from `plugin_config.location` after plugin-folder lookup.
    /// Used for log identity and matching reloads by source path, including installed builtins.
    pub resolved_source_path: Option<PathBuf>,
    pub plugin_cwd: PathBuf,
    // Own the watch with this instance: its root, recipient, and lifetime must agree.
    // ponytail: one watcher per instance; share by canonical root if watcher counts become costly.
    pub(crate) filesystem_watcher: Option<crate::watch_filesystem::FilesystemWatcher>,
    pub session_env_vars: std::collections::BTreeMap<String, String>,
    /// This server's session name, as runtime metadata and plugin warps read it. The
    /// `session_start` hook payload uses a clone of the same handle held by the plugin thread.
    /// See [`crate::ServerSession`] for why it is not a plain name.
    pub server_session: crate::ServerSession,
    /// The `plugins { }` alias table, shared with `PluginBridge` so it stays current across
    /// config reloads. Used by alias configuration queries and plugin loading.
    pub plugin_aliases: Arc<Mutex<PluginAliases>>,
    pub input_pipes_to_unblock: Arc<Mutex<HashSet<String>>>,
    pub input_pipes_to_block: Arc<Mutex<HashSet<String>>>,
    pub default_mode: InputMode,
    pub subscriptions: Arc<Mutex<Subscriptions>>,
    pub keybinds: Keybinds,
    pub intercepting_key_presses: bool,
    /// Where replies to host commands are delivered. See [`ResponseSink`].
    pub response_sink: ResponseSink,
}

impl PluginEnv {
    pub(crate) fn clear_agent_providers(&self) {
        if let Some(generation) = self.generation {
            if yinz_types::project_provider::unregister_owner(generation) {
                crate::yinz_exports::broadcast_project_providers(&self.senders);
            }
            if crate::project_provider::clear(generation) {
                self.senders
                    .send_to_screen(
                        yinz_bus::screen::ScreenInstruction::InvalidateAgentEventProjects,
                    )
                    .context("invalidate project memberships")
                    .non_fatal();
            }
        }

        if self
            .generation
            .is_some_and(yinz_types::agent::provider::unregister_owner)
        {
            crate::yinz_exports::broadcast_agent_providers(&self.senders);
        }
    }

    pub fn originating_plugin(
        &self,
        context: BTreeMap<String, String>,
    ) -> yinz_types::data::OriginatingPlugin {
        yinz_types::data::OriginatingPlugin {
            plugin_id: self.plugin_id,
            client_id: self.client_id,
            generation: self.generation,
            context,
        }
    }

    /// Remove only this runtime's watches, including watches created for other clients.
    pub fn clear_data_watches(&self, generation: u64) {
        use yinz_types::plugin_data::DataError;

        self.watch_generations
            .lock()
            .unwrap()
            .retain(|watch_id, owner| {
                if *owner != generation {
                    return true;
                }
                match self
                    .data_access
                    .plugin_data()
                    .unregister_watch(self.plugin_id, *watch_id)
                {
                    // Tolerate a watch that was already removed.
                    Ok(()) | Err(DataError::InvalidQuery(_)) => false,
                    Err(error) => {
                        Err::<(), _>(error)
                            .context("failed to remove plugin data watch")
                            .non_fatal();
                        // A surviving watch must keep its generation, never become unscoped.
                        true
                    },
                }
            });
    }

    pub fn log_identity(&self) -> String {
        crate::plugin_authoring::LogIdentity::new(
            &self.plugin_config.location,
            self.resolved_source_path
                .as_deref()
                .unwrap_or(&self.plugin_config.path),
            &self.plugin_cwd,
        )
        .marker()
    }

    // Get the name (path) of the containing plugin
    pub fn name(&self) -> String {
        format!(
            "{} (ID {}) {}",
            self.plugin_config.path.to_string_lossy().escape_default(),
            self.plugin_id,
            self.log_identity()
        )
    }

    pub fn set_permissions(&mut self, permissions: HashSet<PermissionType>) {
        self.permissions.lock().unwrap().replace(permissions);
    }
}

#[derive(Eq, PartialEq, Hash)]
pub enum AtomicEvent {
    Resize,
}

/// The newest size the screen asked of one plugin instance, and whether it still owes a frame.
///
/// Every `Resize` the screen sends queues a render on the plugin's pinned executor, and they run
/// in order. Dragging a pane edge sends one per cell the edge crosses, dozens a second, and a
/// plugin whose render takes longer than the gap between them used to replay every one of those
/// sizes after the drag had ended - seconds of backlog in front of the user's next key. The
/// screen writes the size here instead, and whichever render runs next - the first queued resize
/// job, or an event, timer or pipe render that gets there before it - draws at it and pays the
/// frame off; the resize jobs behind it find nothing owed and return without the guest.
///
/// A frame is owed whenever the asked size changes, even back to the one last drawn: the screen
/// resized the pane in between, so a drag that ends where it began still needs one. An
/// instance's first resize draws regardless, as it always has - a plugin can change what it
/// shows before then without asking for a render, and that resize is where it gets one.
///
/// A job is queued only when a request finds none queued already: the one in the queue draws
/// whatever size is newest when it runs, so a second would only find nothing owed. A drag over
/// a busy plugin therefore queues one job, not one per cell the edge crosses.
pub struct ResizeSlot(Mutex<ResizeState>);

struct ResizeState {
    /// (rows, columns)
    wanted: (usize, usize),
    render_owed: bool,
    first_resize_ran: bool,
    /// A resize job is queued for this instance and has not begun.
    job_queued: bool,
}

impl ResizeSlot {
    pub fn new(rows: usize, columns: usize) -> Self {
        Self(Mutex::new(ResizeState {
            wanted: (rows, columns),
            render_owed: false,
            first_resize_ran: false,
            job_queued: false,
        }))
    }

    /// Ask for `rows` x `columns`; asking again for the size already asked owes nothing.
    ///
    /// Answers whether the caller should queue a resize job: one is due, and none is queued to
    /// draw it. A `true` counts the caller's job as queued until it calls [`Self::begin_job`],
    /// or the caller calls [`Self::cancel_job`] because the executor refused it.
    pub fn request(&self, rows: usize, columns: usize) -> bool {
        let mut state = self.state();
        if state.wanted != (rows, columns) {
            state.wanted = (rows, columns);
            state.render_owed = true;
        }
        let enqueue = state.due() && !state.job_queued;
        if enqueue {
            state.job_queued = true;
        }
        enqueue
    }

    /// A queued resize job has begun: the next request queues another. Answers
    /// [`Self::resize_due`], read under the same lock.
    pub fn begin_job(&self) -> bool {
        let mut state = self.state();
        state.job_queued = false;
        state.due()
    }

    /// The job a `true` from [`Self::request`] counted as queued was refused by the executor:
    /// the next request queues another rather than waiting on one that will never begin. The
    /// frame stays owed.
    pub fn cancel_job(&self) {
        self.state().job_queued = false;
    }

    /// Whether a resize job has anything to draw: a frame owed, or its instance's first resize.
    pub fn resize_due(&self) -> bool {
        self.state().due()
    }

    /// A resize job is about to draw; the first resize is no longer to come.
    pub fn note_resize(&self) {
        self.state().first_resize_ran = true;
    }

    /// The size to draw at, clearing the frame owed. A request that lands after this owes a
    /// new one, and the job queued with it draws it.
    pub fn take_for_render(&self) -> (usize, usize) {
        let mut state = self.state();
        state.render_owed = false;
        state.wanted
    }

    /// The newest size asked, leaving any frame owed in place.
    pub fn wanted(&self) -> (usize, usize) {
        self.state().wanted
    }

    // Plain fields, written together and never around guest code: a poisoned lock still
    // holds a consistent state.
    fn state(&self) -> std::sync::MutexGuard<'_, ResizeState> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl ResizeState {
    fn due(&self) -> bool {
        self.render_owed || !self.first_resize_ran
    }
}

pub struct RunningPlugin {
    pub instance_id: PluginInstanceId,
    /// Last config supplied to this instance; startup config remains its reload identity.
    pub current_configuration: PluginUserConfiguration,
    pipes_released: bool,
    pub failed: bool,
    pub pipe_requests: HashMap<String, yinz_bus::plugins::PipeRequestId>,
    runtime: SteelPlugin,
    pub rows: usize,
    pub columns: usize,
    next_event_ids: HashMap<AtomicEvent, usize>,
    last_applied_event_ids: HashMap<AtomicEvent, usize>,
    /// The size the screen last asked of this instance, which `rows` and `columns` catch up to
    /// at the next render. See [`ResizeSlot`].
    pub resize: Arc<ResizeSlot>,
    /// Event types this plugin has already been told it lacks the permission for. A denial is a
    /// standing misconfiguration, not an incident: subscribing to a high-frequency event without
    /// its permission would otherwise log once per keystroke and blow the log's 100KB cap in
    /// seconds. See `note_denied_event`.
    denied_events_logged: HashSet<EventType>,
}

impl RunningPlugin {
    pub fn new(steel: SteelPlugin, rows: usize, columns: usize) -> Self {
        Self {
            instance_id: steel.env().instance_id,
            current_configuration: steel
                .env()
                .plugin_config
                .initial_userspace_configuration
                .clone(),
            pipes_released: false,
            failed: false,
            pipe_requests: HashMap::new(),
            runtime: steel,
            rows,
            columns,
            next_event_ids: HashMap::new(),
            last_applied_event_ids: HashMap::new(),
            resize: Arc::new(ResizeSlot::new(rows, columns)),
            denied_events_logged: HashSet::new(),
        }
    }

    pub fn generation(&self) -> Option<u64> {
        (!self.runtime.is_disposed()).then(|| self.runtime.generation())
    }

    pub fn mark_failed(&mut self) {
        self.failed = true;
        self.env().unavailable.store(true, Ordering::Release);
    }

    pub fn is_unavailable(&self) -> bool {
        self.is_disposed() || self.env().unavailable.load(Ordering::Acquire)
    }

    pub fn is_disposed(&self) -> bool {
        self.pipes_released || self.runtime.is_disposed()
    }

    pub fn plugin_palette(
        &mut self,
        message: &yinz_types::command_palette::Message,
    ) -> Result<bool> {
        self.runtime.palette(message)
    }

    pub fn plugin_tick(&mut self) -> Result<bool> {
        self.runtime.tick()
    }

    /// Release thread-bound runtime state while still on the assigned executor.
    pub fn dispose(&mut self) {
        self.runtime.dispose_with_cleanup(!self.failed);
        self.env().unavailable.store(true, Ordering::Release);
        self.env_mut().filesystem_watcher.take();
        if !self.pipes_released {
            self.pipes_released = true;
            self.env()
                .senders
                .send_to_plugin(PluginInstruction::ReleasePluginInstancePipes(
                    self.env().plugin_id,
                    self.instance_id,
                ))
                .context("failed to release disposed instance's pipes")
                .non_fatal();
        }
    }

    /// Whether the newest size asked of this instance has any area to draw an event's render
    /// into. A resize draws at any size, as it always has.
    pub fn has_area(&self) -> bool {
        let (rows, columns) = self.resize.wanted();
        rows > 0 && columns > 0
    }
    /// Bring `rows` and `columns` up to the newest size asked of this instance, as a render is
    /// about to draw at them, and return them. That render pays off the frame the resize owed.
    pub fn take_size_for_render(&mut self) -> (usize, usize) {
        let (rows, columns) = self.resize.take_for_render();
        self.rows = rows;
        self.columns = columns;
        (rows, columns)
    }
    /// Record that an event of this type was denied to this plugin, returning whether it is the
    /// first of its type - which is the only one worth logging.
    pub fn note_denied_event(&mut self, event_type: EventType) -> bool {
        self.denied_events_logged.insert(event_type)
    }

    pub fn env(&self) -> &PluginEnv {
        self.runtime.env()
    }
    pub fn env_mut(&mut self) -> &mut PluginEnv {
        self.runtime.env_mut()
    }

    /// Call the plugin's `load` entrypoint with its user configuration.
    ///
    /// Takes the `PluginUserConfiguration` rather than its inner map on purpose:
    /// `PluginUserConfiguration::new` strips reserved keys, so rebuilding one here would quietly
    /// drop configuration the plugin was meant to receive.
    pub fn load(&mut self, configuration: &PluginUserConfiguration) -> Result<()> {
        self.runtime.load(configuration.inner())
    }

    /// Call the plugin's `update` entrypoint, returning whether it asked to be re-rendered.
    ///
    /// An event the host cannot encode is deliberately non-fatal: it is logged and dropped
    /// rather than returned as an error, because the caller turns any error here into a plugin
    /// crash and the plugin is not the thing that failed.
    pub fn update(&mut self, event: &Event) -> Result<bool> {
        self.runtime.update(event)
    }

    /// Call the plugin's `pipe` entrypoint, returning whether it asked to be re-rendered.
    /// `Ok(None)` means the plugin does not implement `pipe` at all, which is not an error.
    pub fn pipe(&mut self, message: &PipeMessage) -> Result<Option<bool>> {
        self.runtime.pipe(message).map(Some)
    }

    /// Call the plugin's `render` entrypoint and collect the frame it produced. The bytes are a
    /// full-screen ANSI frame - the pane replaces its whole viewport with them.
    pub fn render(&mut self, rows: usize, columns: usize) -> Result<Vec<u8>> {
        self.runtime.render(rows, columns)
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
#[path = "unit/plugin_map.rs"]
mod tests;

#[cfg(test)]
#[path = "unit/plugin_map_tests.rs"]
mod plugin_map_tests;
