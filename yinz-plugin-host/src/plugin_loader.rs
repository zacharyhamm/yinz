use crate::plugin_bridge::LoadingContext;
use crate::plugin_map::{PluginEnv, PluginMap, ResponseSink, RunningPlugin, CANCELLED_BY_HOST};
use crate::queues::Queues;
use crate::{ClientId, PluginId};
use std::{
    collections::{HashMap, HashSet},
    fs,
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
    yinz_config::plugins::PluginConfig, yinz_runtime::consts::YINZ_TMP_DIR,
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
    if let Err(e) = fs::create_dir_all(YINZ_TMP_DIR.as_path()) {
        log::error!("Failed to create plugin tmp dir: {}", e);
    }
}

fn script_source(
    plugin_config: &PluginConfig,
    user_plugin_dir: Option<&Path>,
) -> Result<(Option<PathBuf>, String)> {
    match &plugin_config.location {
        RunPluginLocation::Yinz(tag) if plugin_config.is_builtin() => {
            let tag = tag.to_string();
            let source = yinz_steel_builtins::builtin_steel_plugin(&tag)
                .with_context(|| format!("the Steel builtin '{tag}' is not compiled in"))?;
            Ok((None, source.to_owned()))
        },
        _ => {
            let (path, source) = plugin_config.resolve_source(user_plugin_dir)?;
            Ok((Some(path.canonicalize().unwrap_or(path)), source))
        },
    }
}

pub struct PluginLoader {
    plugin_roles: crate::plugin_map::PluginRoles,
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
    default_mode: InputMode,
    keybinds: Keybinds,
    configuration_version: u64,
    data_access: Arc<DataAccess>,
    watch_generations: super::plugin_map::WatchGenerations,
    queues: Arc<Mutex<Queues>>,
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
            plugin_roles: loading_context.plugin_roles,
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
            data_access: loading_context.data_access,
            watch_generations: loading_context.watch_generations,
            queues: loading_context.queues,
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
        let (resolved_source_path, source) =
            script_source(&self.plugin_config, self.user_plugin_dir.as_deref())
                .with_context(err_context)?;

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

    fn create_plugin_env(&self) -> PluginEnv {
        PluginEnv {
            background: self
                .plugin_roles
                .lock()
                .unwrap()
                .get(&self.plugin_id)
                .copied()
                .unwrap_or(false),
            plugin_roles: self.plugin_roles.clone(),
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
            queues: self.queues.clone(),
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
        if !connected_clients.is_empty() {
            self.connected_clients = None; // so we don't have infinite loops
            for client_id in connected_clients {
                if client_id == self.client_id {
                    // don't reload the plugin once more for ourselves
                    continue;
                }
                self.client_id = client_id;
                self.client_cancel = crate::plugin_bridge::client_cancel_token(
                    &self.client_cancellations,
                    client_id,
                );
                self.start_plugin()?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::script_source;
    use std::path::Path;
    use yinz_config::plugins::PluginConfig;
    use yinz_types::data::PluginTag;
    use yinz_types::input::layout::{RunPlugin, RunPluginLocation};

    fn builtin_plugin(name: &str) -> PluginConfig {
        PluginConfig::from_run_plugin(&RunPlugin {
            location: RunPluginLocation::Yinz(PluginTag::new(name)),
            ..Default::default()
        })
        .unwrap()
    }

    #[test]
    fn a_builtin_beats_a_same_named_file_in_the_user_plugin_dir() {
        let plugin_dir = tempfile::tempdir().unwrap();
        std::fs::write(plugin_dir.path().join("agent-exit.scm"), "; impostor").unwrap();
        let (path, source) =
            script_source(&builtin_plugin("agent-exit"), Some(plugin_dir.path())).unwrap();
        assert_eq!(path, None, "a builtin has no file of its own");
        assert!(source.contains("Agent exited."));
    }

    #[test]
    fn builtins_resolve_from_embedded_source() {
        for name in yinz_runtime::consts::BUILTIN_PLUGIN_NAMES {
            let plugin = builtin_plugin(name);
            assert!(plugin.is_builtin());
            assert_eq!(
                script_source(&plugin, Some(Path::new("/missing/plugins"))).unwrap(),
                (
                    None,
                    yinz_steel_builtins::builtin_steel_plugin(name)
                        .unwrap()
                        .to_owned()
                )
            );
        }
        let file = PluginConfig::from_run_plugin(&RunPlugin {
            location: RunPluginLocation::File("context-menu.scm".into()),
            ..Default::default()
        })
        .unwrap();
        assert!(!file.is_builtin());
    }

    #[test]
    fn source_resolves_the_file_and_reads_edits_on_reload() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("plugin.scm");
        let config = PluginConfig::from_run_plugin(&RunPlugin {
            location: RunPluginLocation::File(path.clone()),
            ..Default::default()
        })
        .unwrap();
        for source in ["(define title \"first\")", "(define title \"second\")"] {
            std::fs::write(&path, source).unwrap();
            assert_eq!(
                script_source(&config, None).unwrap(),
                (Some(path.canonicalize().unwrap()), source.to_owned())
            );
        }
    }
}
