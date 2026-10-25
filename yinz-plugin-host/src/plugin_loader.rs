use crate::plugin_bridge::LoadingContext;
use crate::plugin_map::{PluginEnv, PluginMap, ResponseSink, RunningPlugin, CANCELLED_BY_HOST};
use crate::plugin_panes::Surface;
use crate::{ClientId, PluginId};
use std::{
    collections::{HashMap, HashSet},
    fs,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use yinz_bus::{loading_indication::LoadingIndication, thread_bus::ThreadSenders};
use yinz_runtime::errors::prelude::*;
use yinz_storage::data_access::DataAccess;
use yinz_types::{internal_ids::TabId, pane_size::Size};
use {
    yinz_config::plugins::PluginConfig, yinz_runtime::consts::YINZ_PLUGIN_TMP_DIR,
    yinz_types::data::InputMode, yinz_types::input::command::TerminalAction,
    yinz_types::input::keybinds::Keybinds, yinz_types::input::layout::RunPluginLocation,
    yinz_types::input::plugins::PluginAliases,
};

fn create_plugin_fs_entries(plugin_own_data_dir: &PathBuf, plugin_own_cache_dir: &PathBuf) {
    // Prepare the roots used by permission-gated filesystem commands.
    // We create them here to get expressive error messages in case they fail.
    if let Err(e) = fs::create_dir_all(plugin_own_data_dir) {
        log::error!("Failed to create plugin data dir: {}", e);
    };
    if let Err(e) = fs::create_dir_all(plugin_own_cache_dir) {
        log::error!("Failed to create plugin cache dir: {}", e);
    }
    // `/tmp` is shared by every plugin of this user, and by no one else.
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(YINZ_PLUGIN_TMP_DIR.as_path())
        .context("failed to create plugin tmp dir")
        .non_fatal();
}

pub(crate) fn script_source(
    plugin_config: &PluginConfig,
    user_plugin_dir: Option<&Path>,
    data_dir: &Path,
) -> Result<(Option<PathBuf>, String)> {
    match &plugin_config.location {
        RunPluginLocation::Yinz(tag) if plugin_config.is_builtin() => {
            let path = yinz_steel_builtins::builtin_path(data_dir, &tag.to_string())?;
            let root = yinz_steel::modules::SourceRoot::file(&path, None)?;
            let source = root.read_entry()?;
            Ok((Some(path), source))
        },
        _ => {
            let (path, source) = plugin_config.resolve_source(user_plugin_dir)?;
            Ok((Some(path.canonicalize().unwrap_or(path)), source))
        },
    }
}

/// What a pane declaration amounts to, in the words of the errors below.
fn describe_pane(declared: bool) -> &'static str {
    if declared {
        "a pane"
    } else {
        "no pane"
    }
}

/// The load error for a module whose evaluated declaration differs from the scan of its source.
pub(crate) fn pane_declaration_mismatch(scanned: bool, evaluated: bool) -> String {
    format!(
        "(yinz-declare-pane) must appear literally at the top level of the entry file (found {}, \
         evaluated {})",
        describe_pane(scanned),
        describe_pane(evaluated)
    )
}

/// The error for an instance whose entry file no longer declares what its plugin was loaded
/// with.
fn pane_declaration_changed(loaded: bool, declared: bool) -> String {
    format!(
        "the entry file's pane declaration changed since the plugin was loaded (loaded with {}, \
         the file now declares {}); reload the plugin to apply it",
        describe_pane(loaded),
        describe_pane(declared)
    )
}

pub struct PluginLoader {
    panes: crate::plugin_panes::PluginPanes,
    /// The entry source the prepare step read for the pane declaration, so the module
    /// evaluated is the one that was scanned. Shared rather than copied: the load evaluates it
    /// once for every connected client.
    prefetched_source: Option<(Option<PathBuf>, Arc<str>)>,
    /// The pane declaration the prepare step scanned in `prefetched_source`.
    prefetched_declaration: Option<bool>,
    pick_request: Option<yinz_types::pick::PickRequest>,
    pick_cancelled: Option<Arc<AtomicBool>>,
    pick_opening: Option<Arc<AtomicBool>>,
    plugin_id: PluginId,
    client_id: ClientId,
    plugin_cwd: PathBuf,
    plugin_own_data_dir: PathBuf,
    temporary_data_dir: Arc<yinz_runtime::owned_dir::OwnedDir>,
    plugin_own_cache_dir: PathBuf,
    plugin_config: PluginConfig,
    data_owner: url::Url,
    tab_index: Option<TabId>,
    path_to_default_shell: PathBuf,
    session_env_vars: std::collections::BTreeMap<String, String>,
    server_session: crate::ServerSession,
    plugin_aliases: Arc<Mutex<PluginAliases>>,
    default_shell: Option<TerminalAction>,
    layout_dir: Option<PathBuf>,
    user_plugin_dir: Option<PathBuf>,
    yinz_data_dir: PathBuf,
    default_mode: InputMode,
    keybinds: Keybinds,
    configuration_version: u64,
    _pending_configuration: Arc<crate::plugin_map::PendingConfiguration>,
    data_access: Arc<DataAccess>,
    watch_generations: super::plugin_map::WatchGenerations,
    size: Size,
    cancel: Arc<AtomicBool>,
    client_cancel: Arc<AtomicBool>,
    client_cancellations: Arc<Mutex<HashMap<ClientId, Arc<AtomicBool>>>>,
    loading_indication: LoadingIndication,
    senders: ThreadSenders,
    plugin_map: Arc<Mutex<PluginMap>>,
    connected_clients: Option<Arc<Mutex<Vec<ClientId>>>>,
    #[cfg(test)]
    pub(crate) before_register: Option<Box<dyn FnOnce() + Send>>,
    #[cfg(test)]
    pub(crate) after_peer_snapshot: Option<Box<dyn FnOnce() + Send>>,
}

impl PluginLoader {
    pub fn new(
        loading_context: LoadingContext,
        senders: ThreadSenders,
        plugin_map: Arc<Mutex<PluginMap>>,
        connected_clients: Arc<Mutex<Vec<ClientId>>>,
    ) -> Self {
        let loading_indication = LoadingIndication::new("".into());
        create_plugin_fs_entries(
            &loading_context.plugin_own_data_dir,
            &loading_context.plugin_own_cache_dir,
        );
        Self {
            yinz_data_dir: loading_context.yinz_data_dir.clone(),
            panes: loading_context.panes,
            prefetched_source: loading_context.prefetched_source,
            prefetched_declaration: loading_context.prefetched_declaration,
            pick_request: loading_context.pick_request,
            pick_cancelled: loading_context.pick_cancelled,
            pick_opening: loading_context.pick_opening,
            plugin_id: loading_context.plugin_id,
            client_id: loading_context.client_id,
            plugin_cwd: loading_context.plugin_cwd,
            plugin_own_data_dir: loading_context.plugin_own_data_dir,
            temporary_data_dir: loading_context.temporary_data_dir,
            plugin_own_cache_dir: loading_context.plugin_own_cache_dir,
            plugin_config: loading_context.plugin_config,
            data_owner: loading_context.data_owner,
            tab_index: loading_context.tab_index,
            path_to_default_shell: loading_context.path_to_default_shell,
            session_env_vars: loading_context.session_env_vars,
            server_session: loading_context.server_session,
            plugin_aliases: loading_context.plugin_aliases,
            default_shell: loading_context.default_shell,
            layout_dir: loading_context.layout_dir,
            user_plugin_dir: loading_context.user_plugin_dir,
            default_mode: loading_context.default_mode,
            keybinds: loading_context.keybinds,
            configuration_version: loading_context.configuration_version,
            _pending_configuration: loading_context.pending_configuration,
            data_access: loading_context.data_access,
            watch_generations: loading_context.watch_generations,
            size: loading_context.size,
            cancel: loading_context.cancel,
            client_cancel: loading_context.client_cancel,
            client_cancellations: loading_context.client_cancellations,

            senders,
            plugin_map,
            connected_clients: Some(connected_clients),
            loading_indication,
            #[cfg(test)]
            before_register: None,
            #[cfg(test)]
            after_peer_snapshot: None,
        }
    }

    pub fn without_connected_clients(mut self) -> Self {
        self.connected_clients = None;
        self
    }

    pub fn start_plugin(&mut self) -> Result<()> {
        {
            let mut plugin_map = self.plugin_map.lock().unwrap();
            anyhow::ensure!(
                plugin_map.load_is_current(self.plugin_id, &self.cancel),
                CANCELLED_BY_HOST
            );
            // An attachment can initialize this client before the initial load reaches its peers.
            if plugin_map.contains(self.plugin_id, self.client_id) {
                drop(plugin_map);
                return self.clone_instance_for_other_clients();
            }
            let resolved_source_path = plugin_map
                .reload_metadata(self.plugin_id)
                .filter(|metadata| {
                    metadata.config.path == self.plugin_config.path
                        && metadata.config.location == self.plugin_config.location
                })
                .and_then(|metadata| metadata.resolved_source_path);
            plugin_map.remember_plugin(
                self.plugin_id,
                super::plugin_map::PluginMetadata {
                    config: self.plugin_config.clone(),
                    data_owner: self.data_owner.clone(),
                    resolved_source_path,
                    cache_dir: self.plugin_own_cache_dir.clone(),
                    cwd: self.plugin_cwd.clone(),
                    tab_index: self.tab_index,
                    size: (self.size.rows, self.size.cols),
                    owner_client_id: self.client_id,
                },
            );
        }
        if self
            .plugin_config
            .path
            .extension()
            .and_then(|ext| ext.to_str())
            == Some("lua")
        {
            anyhow::bail!("Lua plugins are no longer supported; migrate to Steel Scheme");
        }
        self.start_script_plugin()
    }

    fn start_script_plugin(&mut self) -> Result<()> {
        if self.script_client_disconnected() {
            return self.clone_instance_for_other_clients();
        }
        let err_context = || {
            format!(
                "failed to start the script plugin at '{}'",
                self.plugin_config.path.display()
            )
        };
        self.loading_indication.override_previous_error();
        // A load, and the instances it clones for the other clients, evaluate the source its
        // declaration was scanned from. An instance for a client attaching to a running plugin
        // reads the entry again, as it always has, so that it runs what is on disk.
        let (resolved_source_path, source): (_, Arc<str>) = match &self.prefetched_source {
            Some((path, source)) => (path.clone(), source.clone()),
            None => {
                let (path, source) = script_source(
                    &self.plugin_config,
                    self.user_plugin_dir.as_deref(),
                    &self.yinz_data_dir,
                )
                .with_context(err_context)?;
                self.check_declaration_unchanged(&source)
                    .with_context(err_context)?;
                (path, source.into())
            },
        };

        let mut plugin_env = self.create_plugin_env();
        plugin_env.resolved_source_path = resolved_source_path.clone();
        {
            let mut plugin_map = self.plugin_map.lock().unwrap();
            anyhow::ensure!(
                plugin_map.load_is_current(self.plugin_id, &self.cancel),
                CANCELLED_BY_HOST
            );
            if let Some(mut metadata) = plugin_map.reload_metadata(self.plugin_id) {
                metadata.resolved_source_path = plugin_env.resolved_source_path.clone();
                plugin_map.remember_plugin(self.plugin_id, metadata);
            }
        }
        let subscriptions = plugin_env.subscriptions.clone();
        let running = {
            // A builtin runs under its bare tag, which is what its `path` holds.
            let path = resolved_source_path.unwrap_or_else(|| self.plugin_config.path.clone());
            let client_cancel = if self.plugin_config.is_shared() {
                Arc::default()
            } else {
                self.client_cancel.clone()
            };
            let mut steel =
                match super::steel::SteelPlugin::new(&source, &path, plugin_env, client_cancel) {
                    Ok(steel) => steel,
                    Err(_) if self.script_client_disconnected() => {
                        return self.clone_instance_for_other_clients();
                    },
                    Err(error) => return Err(error).with_context(err_context),
                };
            if let Err(error) = self.check_pane_declaration(&mut steel) {
                steel.dispose_with_cleanup(false);
                return Err(error).with_context(err_context);
            }
            let workers_started = (|| -> Result<()> {
                for name in steel.worker_names()? {
                    if let Some(workers) = &steel.env().workers {
                        workers.spawn(name, &source, &path, self.worker_env(steel.env()))?;
                    }
                }
                Ok(())
            })();
            if let Err(error) = workers_started {
                steel.dispose_with_cleanup(false);
                if self.script_client_disconnected() {
                    return self.clone_instance_for_other_clients();
                }
                return Err(error).with_context(err_context);
            }
            RunningPlugin::new(steel, self.size.rows, self.size.cols)
        };
        let plugin = Arc::new(Mutex::new(running));
        #[cfg(test)]
        if let Some(before_register) = self.before_register.take() {
            before_register();
        }
        // Inserted before `load` runs, so that host commands issued from `load` can find it.
        let previous = {
            let mut plugin_map = self.plugin_map.lock().unwrap();
            let load_is_current = plugin_map.load_is_current(self.plugin_id, &self.cancel);
            if !load_is_current || self.script_client_disconnected() {
                drop(plugin_map);
                let mut running = plugin.lock().unwrap();
                running.mark_failed();
                running.dispose();
                drop(running);
                anyhow::ensure!(load_is_current, CANCELLED_BY_HOST);
                return self.clone_instance_for_other_clients();
            }
            if let Some((version, configuration)) =
                plugin_map.client_configurations.get(&self.client_id)
            {
                // Older updates must not overwrite the bridge defaults captured by a newer context.
                if *version > self.configuration_version {
                    configuration.apply_to(&mut plugin.lock().unwrap());
                }
            }
            plugin_map.insert(
                self.plugin_id,
                self.client_id,
                plugin.clone(),
                subscriptions,
            )
        };
        if let Some((previous, _)) = previous {
            previous.lock().unwrap().dispose();
        }
        let loaded = plugin
            .lock()
            .unwrap()
            .load(&self.plugin_config.initial_userspace_configuration)
            .with_context(err_context);
        if loaded.is_err()
            || self.cancel.load(Ordering::Acquire)
            || self.script_client_disconnected()
        {
            {
                let mut running = plugin.lock().unwrap();
                running.mark_failed();
                running.dispose();
            }
            self.plugin_map
                .lock()
                .unwrap()
                .remove_matching_instance(self.plugin_id, &plugin);
            anyhow::ensure!(!self.cancel.load(Ordering::Acquire), CANCELLED_BY_HOST);
            if self.script_client_disconnected() {
                return self.clone_instance_for_other_clients();
            }
        }
        loaded?;

        self.clone_instance_for_other_clients()?;
        Ok(())
    }

    /// An instance that read the entry again must find the declaration the plugin's pane was
    /// decided on. The plugin has one surface for every client, so an entry whose declaration
    /// changed since the plugin was loaded can only take effect through a reload. Keeping the
    /// loaded entry instead would not do: the instance would pair it with the imports as they
    /// are now, a version of the plugin that never existed on disk.
    ///
    /// An entry Steel cannot read is left to evaluation, which names its syntax error: what such
    /// source declares is only a guess, and a mismatch of the guess would hide that error.
    fn check_declaration_unchanged(&self, source: &str) -> Result<()> {
        let Some(loaded) = self.panes.lock().unwrap().scanned(self.plugin_id) else {
            return Ok(());
        };
        let Some(declaration) = yinz_steel::manifest::readable_pane_declaration(source)
            .context("reading the plugin's pane declaration")?
        else {
            return Ok(());
        };
        let declared =
            declaration.applies(self.plugin_config.initial_userspace_configuration.inner());
        if declared != loaded {
            bail!("{}", pane_declaration_changed(loaded, declared));
        }
        Ok(())
    }

    /// The evaluated module must declare what the prepare step's scan found: a declaration
    /// inside `begin` or `when`, in an imported module or made by a macro is invisible to the
    /// scan, and the plugin's pane was already decided without it.
    ///
    /// A load checks against the scan of the source it evaluates. Another client's instance,
    /// which read the entry again and found the same declaration there, checks against the
    /// plugin's.
    fn check_pane_declaration(&self, steel: &mut super::steel::SteelPlugin) -> Result<()> {
        let scanned = self
            .prefetched_declaration
            .or_else(|| self.panes.lock().unwrap().scanned(self.plugin_id));
        let Some(scanned) = scanned else {
            return Ok(());
        };
        let evaluated =
            steel.pane_declared(self.plugin_config.initial_userspace_configuration.inner())?;
        if evaluated != scanned {
            bail!("{}", pane_declaration_mismatch(scanned, evaluated));
        }
        Ok(())
    }

    fn script_client_disconnected(&self) -> bool {
        !self.plugin_config.is_shared() && self.client_cancel.load(Ordering::Acquire)
    }

    fn worker_env(&self, parent: &PluginEnv) -> PluginEnv {
        let mut env = self.create_plugin_env();
        env.instance_id = parent.instance_id;
        env.permissions = parent.permissions.clone();
        env.permissions_requested = parent.permissions_requested.clone();
        env.resolved_source_path = parent.resolved_source_path.clone();
        env.workers = parent.workers.clone();
        env
    }

    /// Whether the plugin has a pane, decided by the prepare step before this load was queued.
    fn surface(&self) -> Surface {
        self.panes
            .lock()
            .unwrap()
            .surface(self.plugin_id)
            .unwrap_or_else(|| {
                log::error!(
                    "plugin {} is loading before its pane declaration was read; it runs headless",
                    self.plugin_id
                );
                Surface::Headless
            })
    }

    fn create_plugin_env(&self) -> PluginEnv {
        // Read before the literal: a guard taken inside it would be held to its end.
        let background = self.panes.lock().unwrap().background(self.plugin_id);
        let surface = self.surface();
        PluginEnv {
            yinz_data_dir: self.yinz_data_dir.clone(),
            background,
            surface,
            panes: self.panes.clone(),
            pick_request: self.pick_request.clone(),
            pick_cancelled: self.pick_cancelled.clone(),
            pick_opening: self.pick_opening.clone(),
            response_sink: ResponseSink::default(),
            plugin_id: self.plugin_id,
            instance_id: Default::default(),
            unavailable: Default::default(),
            cancel: self.cancel.clone(),
            cancel_tripped: Arc::new(AtomicBool::new(false)),
            generation: None,
            session_preparations: Default::default(),
            workers: None,
            client_id: self.client_id,
            owner_client_id: self.client_id,
            plugin_config: self.plugin_config.clone(),
            data_owner: self.data_owner.clone(),
            permissions: Arc::new(Mutex::new(None)),
            permissions_requested: Arc::new(AtomicBool::new(false)),
            senders: self.senders.clone(),
            plugin_own_data_dir: self.plugin_own_data_dir.clone(),
            temporary_data_dir: Some(self.temporary_data_dir.clone()),
            data_access: self.data_access.clone(),
            watch_generations: self.watch_generations.clone(),
            plugin_own_cache_dir: self.plugin_own_cache_dir.clone(),
            tab_index: self.tab_index,
            path_to_default_shell: self.path_to_default_shell.clone(),
            default_shell: self.default_shell.clone(),
            plugin_cwd: self.plugin_cwd.clone(),
            filesystem_watcher: None,
            session_env_vars: self.session_env_vars.clone(),
            server_session: self.server_session.clone(),
            plugin_aliases: self.plugin_aliases.clone(),
            input_pipes_to_unblock: Arc::new(Mutex::new(HashSet::new())),
            input_pipes_to_block: Arc::new(Mutex::new(HashSet::new())),
            layout_dir: self.layout_dir.clone(),
            user_plugin_dir: self.user_plugin_dir.clone(),
            resolved_source_path: None,
            default_mode: self.default_mode,
            subscriptions: Arc::new(Mutex::new(HashSet::new())),
            keybinds: self.keybinds.clone(),
            intercepting_key_presses: false,
        }
    }

    pub fn clone_instance_for_other_clients(&mut self) -> Result<()> {
        if self.plugin_config.is_shared() {
            // a shared plugin has exactly one instance for the whole session, owned by the client
            // that loaded it
            return Ok(());
        }
        let Some(connected_clients) = self.connected_clients.as_ref() else {
            return Ok(());
        };
        let connected_clients: Vec<ClientId> =
            connected_clients.lock().unwrap().iter().copied().collect();
        #[cfg(test)]
        if let Some(after_snapshot) = self.after_peer_snapshot.take() {
            after_snapshot();
        }
        if !connected_clients.is_empty() {
            self.connected_clients = None; // so we don't have infinite loops
            for client_id in connected_clients {
                if client_id == self.client_id {
                    // don't reload the plugin once more for ourselves
                    continue;
                }
                let client_cancel = self
                    .client_cancellations
                    .lock()
                    .unwrap()
                    .get(&client_id)
                    .cloned();
                let Some(client_cancel) = client_cancel else {
                    continue;
                };
                self.client_id = client_id;
                self.client_cancel = client_cancel;
                self.start_plugin()?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "unit/plugin_loader.rs"]
mod tests;
