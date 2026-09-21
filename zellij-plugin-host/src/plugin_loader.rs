use crate::plugin_limits::PluginResourceLimits;
use crate::plugin_map::{
    PluginEnv, PluginMap, ResponseSink, RunningPlugin, VecDequeInputStream, WriteOutputStream,
};
use crate::plugin_worker::{plugin_worker, RunningWorker};
use crate::wasm_bridge::{LoadingContext, PluginCache, PluginCacheKey};
use crate::zellij_exports::zellij_exports;
use crate::PluginId;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use wasmi::{Engine, Instance, Linker, Module, Store};
use wasmi_wasi::sync::WasiCtxBuilder;
use wasmi_wasi::wasi_common::pipe::{ReadPipe, WritePipe};
use wasmi_wasi::Dir;
use wasmi_wasi::WasiCtx;
use zellij_types::internal_ids::TabId;

use crate::{logging_pipe::LoggingPipe, ClientId};
use zellij_bus::{loading_indication::LoadingIndication, thread_bus::ThreadSenders};

use zellij_storage::data_access::DataAccess;

use crate::queues::Queues;
use zellij_runtime::errors::prelude::*;
use zellij_types::pane_size::Size;
use {
    zellij_config::plugins::PluginConfig, zellij_config::plugins::PluginRuntime,
    zellij_runtime::consts::ZELLIJ_TMP_DIR, zellij_types::data::InputMode,
    zellij_types::input::command::TerminalAction, zellij_types::input::keybinds::Keybinds,
    zellij_types::input::plugins::PluginAliases,
};

fn open_dir(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    std::fs::File::open(path)
}

fn create_plugin_fs_entries(plugin_own_data_dir: &PathBuf, plugin_own_cache_dir: &PathBuf) {
    // Create filesystem entries mounted into WASM.
    // We create them here to get expressive error messages in case they fail.
    if let Err(e) = fs::create_dir_all(plugin_own_data_dir) {
        log::error!("Failed to create plugin data dir: {}", e);
    };
    if let Err(e) = fs::create_dir_all(plugin_own_cache_dir) {
        log::error!("Failed to create plugin cache dir: {}", e);
    }
    if let Err(e) = fs::create_dir_all(ZELLIJ_TMP_DIR.as_path()) {
        log::error!("Failed to create plugin tmp dir: {}", e);
    }
}

fn plugin_cache_key(
    plugin_config: &PluginConfig,
    builtin_plugins_dir: Option<&Path>,
) -> PluginCacheKey {
    let builtin_plugins_dir = if plugin_config.is_builtin() {
        builtin_plugins_dir.map(Path::to_path_buf)
    } else {
        None
    };
    (plugin_config.path.clone(), builtin_plugins_dir)
}

/// Stop the guest at its next crossing of the host boundary once `PluginEnv::cancel` is set.
///
/// This is what lets an unload or a shutdown get past an instance that is looping inside
/// `update()`: the executor thread it is pinned to is the only thing that can run its cleanup,
/// and that thread is inside the guest. wasmi has no epoch interruption, so the hook fires on
/// each call into or out of wasm — every host command, every WASI read and write — which is
/// where such a loop spends its time. A loop that never calls the host is not caught; that
/// would take fuel metering, which this deliberately does not turn on.
fn install_cancel_hook(store: &mut Store<PluginEnv>) {
    store.call_hook(|plugin_env: &mut PluginEnv, _call| {
        if plugin_env.cancel.load(Ordering::Relaxed) {
            plugin_env.cancel_tripped.store(true, Ordering::Relaxed);
            Err(wasmi::Error::new(crate::plugin_map::CANCELLED_BY_HOST))
        } else {
            Ok(())
        }
    });
}

pub struct PluginLoader<'a> {
    skip_cache: bool,
    plugin_id: PluginId,
    client_id: ClientId,
    plugin_cwd: PathBuf,
    plugin_own_data_dir: PathBuf,
    temporary_data_dir: Arc<zellij_runtime::owned_dir::OwnedDir>,
    plugin_own_cache_dir: PathBuf,
    plugin_config: PluginConfig,
    tab_index: Option<TabId>,
    path_to_default_shell: PathBuf,
    session_env_vars: std::collections::BTreeMap<String, String>,
    plugin_aliases: Arc<Mutex<PluginAliases>>,
    default_shell: Option<TerminalAction>,
    layout_dir: Option<PathBuf>,
    user_plugin_dir: Option<PathBuf>,
    builtin_plugins_dir: Option<PathBuf>,
    default_mode: InputMode,
    keybinds: Keybinds,
    zellij_data_dir: PathBuf,
    data_access: Arc<DataAccess>,
    deno_watch_generations: super::plugin_map::DenoWatchGenerations,
    queues: Arc<Mutex<Queues>>,
    size: Size,
    cancel: Arc<AtomicBool>,
    client_cancel: Arc<AtomicBool>,
    client_cancellations: Arc<Mutex<HashMap<ClientId, Arc<AtomicBool>>>>,
    loading_indication: LoadingIndication,
    senders: ThreadSenders,
    engine: Engine,
    plugin_cache: PluginCache,
    plugin_map: &'a mut PluginMap, // we receive a mutable reference rather than the Arc so that it
    // will be held for the lifetime of this struct and thus loading
    // plugins for all connected clients will be one transaction
    connected_clients: Option<Arc<Mutex<Vec<ClientId>>>>,
}

impl<'a> PluginLoader<'a> {
    pub fn new(
        skip_cache: bool,
        loading_context: LoadingContext,
        senders: ThreadSenders,
        engine: Engine,
        plugin_cache: PluginCache,
        plugin_map: &'a mut PluginMap,
        connected_clients: Arc<Mutex<Vec<ClientId>>>,
    ) -> Self {
        let loading_indication = LoadingIndication::new("".into());
        create_plugin_fs_entries(
            &loading_context.plugin_own_data_dir,
            &loading_context.plugin_own_cache_dir,
        );
        Self {
            plugin_id: loading_context.plugin_id,
            client_id: loading_context.client_id,
            plugin_cwd: loading_context.plugin_cwd,
            plugin_own_data_dir: loading_context.plugin_own_data_dir,
            temporary_data_dir: loading_context.temporary_data_dir,
            plugin_own_cache_dir: loading_context.plugin_own_cache_dir,
            plugin_config: loading_context.plugin_config,
            tab_index: loading_context.tab_index,
            path_to_default_shell: loading_context.path_to_default_shell,
            session_env_vars: loading_context.session_env_vars,
            plugin_aliases: loading_context.plugin_aliases,
            default_shell: loading_context.default_shell,
            layout_dir: loading_context.layout_dir,
            user_plugin_dir: loading_context.user_plugin_dir,
            builtin_plugins_dir: loading_context.builtin_plugins_dir,
            default_mode: loading_context.default_mode,
            keybinds: loading_context.keybinds,
            zellij_data_dir: loading_context.zellij_data_dir,
            data_access: loading_context.data_access,
            deno_watch_generations: loading_context.deno_watch_generations,
            queues: loading_context.queues,
            size: loading_context.size,
            cancel: loading_context.cancel,
            client_cancel: loading_context.client_cancel,
            client_cancellations: loading_context.client_cancellations,

            skip_cache,
            senders,
            engine,
            plugin_cache,
            plugin_map,
            connected_clients: Some(connected_clients),
            loading_indication,
        }
    }
    pub fn without_connected_clients(mut self) -> Self {
        self.connected_clients = None;
        self
    }
    pub fn start_plugin(&mut self) -> Result<()> {
        let resolved_source_path = self
            .plugin_map
            .reload_metadata(self.plugin_id)
            .and_then(|metadata| metadata.resolved_source_path);
        self.plugin_map.remember_plugin(
            self.plugin_id,
            super::plugin_map::PluginMetadata {
                config: self.plugin_config.clone(),
                resolved_source_path,
                cwd: self.plugin_cwd.clone(),
                tab_index: self.tab_index,
                size: (self.size.rows, self.size.cols),
                owner_client_id: self.client_id,
            },
        );
        if self
            .plugin_config
            .path
            .extension()
            .and_then(|ext| ext.to_str())
            == Some("lua")
        {
            anyhow::bail!("Lua plugins are no longer supported; migrate to TypeScript or Wasm");
        }
        if matches!(self.plugin_config.runtime(), PluginRuntime::Deno) {
            return self.start_script_plugin();
        }
        let module = if self.skip_cache {
            self.interpret_module()?
        } else {
            self.load_module_from_memory()
                .or_else(|_e| self.interpret_module())?
        };
        let (store, instance) = self.create_plugin_environment(module)?;
        self.load_plugin_instance(store, &instance)?;
        self.clone_instance_for_other_clients()?;
        Ok(())
    }
    /// Load a script plugin: read its source, build a runtime for it, and register it exactly
    /// as a wasm plugin is registered. Everything downstream of the `PluginMap` - event routing,
    /// rendering, the loading indicator - is shared, so only the two runtime-shaped steps differ.
    ///
    /// Re-reading the source on every load makes editing and reloading scripts work.
    fn start_script_plugin(&mut self) -> Result<()> {
        if self.deno_client_disconnected() {
            return self.clone_instance_for_other_clients();
        }
        let err_context = || {
            format!(
                "failed to start the script plugin at '{}'",
                self.plugin_config.path.display()
            )
        };
        self.loading_indication.override_previous_error();
        let (resolved_path, source) = self
            .plugin_config
            .resolve_source(self.user_plugin_dir.as_deref())
            .with_context(err_context)?;

        // Script plugins have no WASI layer, so they get no wasi context. The pipes go unused, but
        // `PluginEnv` is shared with the wasm runtime and they are cheap.
        let mut plugin_env = self.create_plugin_env(
            None,
            Arc::new(Mutex::new(VecDeque::new())),
            Arc::new(Mutex::new(VecDeque::new())),
        );
        // Which of the two candidate files was actually read - `location` cannot say, and a
        // reload arrives naming this path in full. `create_plugin_env` is shared with the wasm
        // runtime, so it is recorded here rather than threaded through as a parameter.
        plugin_env.resolved_source_path = if self.plugin_config.builtin_deno_source().is_some() {
            None
        } else {
            Some(resolved_path.clone())
        };
        if let Some(mut metadata) = self.plugin_map.reload_metadata(self.plugin_id) {
            metadata.resolved_source_path = plugin_env.resolved_source_path.clone();
            self.plugin_map.remember_plugin(self.plugin_id, metadata);
        }
        let subscriptions = plugin_env.subscriptions.clone();
        let running = {
            let path = resolved_path;
            let client_cancel = if self.plugin_config.is_shared() {
                Arc::default()
            } else {
                self.client_cancel.clone()
            };
            let mut deno =
                match super::deno::DenoPlugin::new(&source, &path, plugin_env, client_cancel) {
                    Ok(deno) => deno,
                    Err(_) if self.deno_client_disconnected() => {
                        return self.clone_instance_for_other_clients()
                    },
                    Err(error) => return Err(error).with_context(err_context),
                };
            for name in deno.worker_names()? {
                let mut env = self.create_plugin_env(None, Arc::default(), Arc::default());
                env.permissions = deno.env().permissions.clone();
                env.permissions_requested = deno.env().permissions_requested.clone();
                env.resolved_source_path = deno.env().resolved_source_path.clone();
                env.deno_workers = deno.env().deno_workers.clone();
                if let Some(workers) = &deno.env().deno_workers {
                    workers.spawn(name, &source, &path, env)?;
                }
            }
            RunningPlugin::new_deno(deno, self.size.rows, self.size.cols)
        };
        let plugin = Arc::new(Mutex::new(running));
        // Inserted before `load` runs, so that host commands issued from `load` can find it.
        self.plugin_map.insert(
            self.plugin_id,
            self.client_id,
            plugin.clone(),
            subscriptions,
            HashMap::new(),
        );
        let loaded = plugin
            .lock()
            .unwrap()
            .load(&self.plugin_config.initial_userspace_configuration)
            .with_context(err_context);
        if loaded.is_err() {
            plugin.lock().unwrap().dispose();
            self.plugin_map
                .remove_instance(self.plugin_id, self.client_id);
            if self.deno_client_disconnected() {
                return self.clone_instance_for_other_clients();
            }
        }
        loaded?;

        self.clone_instance_for_other_clients()?;
        Ok(())
    }
    fn deno_client_disconnected(&self) -> bool {
        self.plugin_config.runtime() == PluginRuntime::Deno
            && !self.plugin_config.is_shared()
            && self.client_cancel.load(Ordering::Acquire)
    }
    fn interpret_module(&mut self) -> Result<Module> {
        self.loading_indication.override_previous_error();
        let wasm_bytes = self
            .plugin_config
            .resolve_wasm_bytes(&self.zellij_data_dir, self.builtin_plugins_dir.as_deref())?;
        let timer = std::time::Instant::now();
        let module = Module::new(&self.engine, &wasm_bytes)?;
        log::info!(
            "Loaded plugin '{}' in {:?}",
            self.plugin_config.path.display(),
            timer.elapsed()
        );
        Ok(module)
    }
    fn load_module_from_memory(&mut self) -> Result<Module> {
        let module = self
            .plugin_cache
            .lock()
            .unwrap()
            .remove(&plugin_cache_key(
                &self.plugin_config,
                self.builtin_plugins_dir.as_deref(),
            )) // TODO: do we still bring it back later?
            // maybe we can forgo this dance?
            .ok_or(anyhow!("Plugin is not stored in memory"))?;
        Ok(module)
    }
    fn load_plugin_instance(
        &mut self,
        mut store: Store<PluginEnv>,
        instance: &Instance,
    ) -> Result<()> {
        let err_context = || format!("failed to load plugin from instance {instance:#?}");
        let main_user_instance = *instance;
        // Resolved here only to fail early: a module missing either export is not a plugin, and
        // we would rather find that out before it is inserted into the `PluginMap`. The calls
        // themselves happen through `RunningPlugin` once it is in the map.
        instance
            .get_typed_func::<(), ()>(&mut store, "_start")
            .with_context(err_context)?;
        instance
            .get_typed_func::<(), ()>(&mut store, "load")
            .with_context(err_context)?;
        let mut workers = HashMap::new();
        // Workers share the plugin's permission cell so grants (which can land
        // after worker creation) apply to them too.
        let plugin_permissions = store.data().permissions.clone();
        for function_name in instance
            .exports(&mut store)
            .filter_map(|export| export.clone().into_func().map(|_| export.name()))
        {
            if function_name.ends_with("_worker") {
                let (mut store, instance) =
                    self.create_plugin_instance_and_wasi_env_for_worker()?;
                store.data_mut().permissions = plugin_permissions.clone();
                let start_function_for_worker = instance
                    .get_typed_func::<(), ()>(&mut store, "_start")
                    .with_context(err_context)?;
                start_function_for_worker
                    .call(&mut store, ())
                    .with_context(err_context)?;

                let worker = RunningWorker::new(store, instance, function_name);
                let worker_sender = plugin_worker(worker);
                workers.insert(function_name.into(), worker_sender);
            }
        }

        let subscriptions = store.data().subscriptions.clone();
        let plugin = Arc::new(Mutex::new(RunningPlugin::new(
            store,
            main_user_instance,
            self.size.rows,
            self.size.cols,
        )));
        self.plugin_map.insert(
            self.plugin_id,
            self.client_id,
            plugin.clone(),
            subscriptions,
            workers,
        );

        let loaded = (|| {
            let mut plugin = plugin.lock().unwrap();
            plugin.start().with_context(err_context)?;
            plugin
                .load(&self.plugin_config.initial_userspace_configuration)
                .with_context(err_context)
        })();
        if loaded.is_err() {
            self.plugin_map
                .remove_instance(self.plugin_id, self.client_id);
        }
        loaded?;

        Ok(())
    }
    /// Build the per-instance environment every host command acts on.
    ///
    /// Shared by both runtimes: `wasi_ctx` and the stdio pipes are the only wasm-shaped parts,
    /// and script plugins pass `None`/fresh pipes for them.
    fn create_plugin_env(
        &self,
        wasi_ctx: Option<WasiCtx>,
        stdin_pipe: Arc<Mutex<VecDeque<u8>>>,
        stdout_pipe: Arc<Mutex<VecDeque<u8>>>,
    ) -> PluginEnv {
        PluginEnv {
            response_sink: ResponseSink::WasiStdin,
            plugin_id: self.plugin_id,
            cancel: self.cancel.clone(),
            cancel_tripped: Arc::new(AtomicBool::new(false)),
            deno_generation: None,
            deno_workers: None,
            client_id: self.client_id,
            owner_client_id: self.client_id,
            plugin: self.plugin_config.clone(), // TODO: change field name in PluginEnv to plugin_config
            permissions: Arc::new(Mutex::new(None)),
            permissions_requested: Arc::new(AtomicBool::new(false)),
            senders: self.senders.clone(),
            wasi_ctx,
            plugin_own_data_dir: self.plugin_own_data_dir.clone(),
            temporary_data_dir: Some(self.temporary_data_dir.clone()),
            data_access: self.data_access.clone(),
            deno_watch_generations: self.deno_watch_generations.clone(),
            queues: self.queues.clone(),
            plugin_own_cache_dir: self.plugin_own_cache_dir.clone(),
            tab_index: self.tab_index,
            path_to_default_shell: self.path_to_default_shell.clone(),
            default_shell: self.default_shell.clone(),
            plugin_cwd: self.plugin_cwd.clone(),
            session_env_vars: self.session_env_vars.clone(),
            plugin_aliases: self.plugin_aliases.clone(),
            input_pipes_to_unblock: Arc::new(Mutex::new(HashSet::new())),
            input_pipes_to_block: Arc::new(Mutex::new(HashSet::new())),
            layout_dir: self.layout_dir.clone(),
            user_plugin_dir: self.user_plugin_dir.clone(),
            // Set by `start_script_plugin` once the source has been resolved; a wasm plugin has
            // its own lookup and leaves it unset.
            resolved_source_path: None,
            default_mode: self.default_mode,
            subscriptions: Arc::new(Mutex::new(HashSet::new())),
            keybinds: self.keybinds.clone(),
            intercepting_key_presses: false,
            stdin_pipe,
            stdout_pipe,
            store_limits: PluginResourceLimits::new(self.plugin_id),
        }
    }

    pub fn create_plugin_environment(
        &self,
        module: Module,
    ) -> Result<(Store<PluginEnv>, Instance)> {
        let err_context = || {
            format!(
                "Failed to create instance, plugin env and subscriptions for plugin {}",
                self.plugin_id
            )
        };
        let stdin_pipe = Arc::new(Mutex::new(VecDeque::new()));
        let stdout_pipe = Arc::new(Mutex::new(VecDeque::new()));

        let wasi_ctx = PluginLoader::create_wasi_ctx(
            &self.plugin_cwd,
            &self.plugin_own_data_dir,
            &self.plugin_own_cache_dir,
            &ZELLIJ_TMP_DIR,
            self.user_plugin_dir.as_ref(),
            &self.plugin_config.location.to_string(),
            self.plugin_id,
            stdin_pipe.clone(),
            stdout_pipe.clone(),
        )?;
        let plugin_cache_key =
            plugin_cache_key(&self.plugin_config, self.builtin_plugins_dir.as_deref());
        let plugin_env =
            self.create_plugin_env(Some(wasi_ctx), stdin_pipe.clone(), stdout_pipe.clone());
        let mut store = Store::new(&self.engine, plugin_env);
        install_cancel_hook(&mut store);

        // See `plugin_limits` for what a plugin is allowed to allocate
        store.limiter(|plugin_env| &mut plugin_env.store_limits);

        let mut linker = Linker::new(&self.engine);
        wasmi_wasi::add_to_linker(&mut linker, |plugin_env: &mut PluginEnv| {
            plugin_env
                .wasi_ctx
                .as_mut()
                .expect("a wasm plugin always has a WASI context")
        })?;
        zellij_exports(&mut linker);

        let instance = linker
            .instantiate_and_start(&mut store, &module)
            .with_context(err_context)?;

        if let Some(func) = instance.get_func(&mut store, "_initialize") {
            if let Ok(typed_func) = func.typed::<(), ()>(&store) {
                let _ = typed_func.call(&mut store, ());
            }
        }

        self.plugin_cache
            .lock()
            .unwrap()
            .insert(plugin_cache_key, module);
        Ok((store, instance))
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
                self.client_cancel = self
                    .client_cancellations
                    .lock()
                    .unwrap()
                    .entry(client_id)
                    .or_default()
                    .clone();
                self.start_plugin()?;
            }
        }
        Ok(())
    }
    pub fn create_plugin_instance_and_wasi_env_for_worker(
        &self,
    ) -> Result<(Store<PluginEnv>, Instance)> {
        let plugin_id = self.plugin_id;
        let err_context = || {
            format!(
                "Failed to create instance and plugin env for worker {}",
                plugin_id
            )
        };
        let module = self
            .plugin_cache
            .lock()
            .unwrap()
            .get(&plugin_cache_key(
                &self.plugin_config,
                self.builtin_plugins_dir.as_deref(),
            ))
            .with_context(err_context)?
            .clone();
        let (store, instance) = self.create_plugin_instance_env(&module)?;
        Ok((store, instance))
    }
    fn create_plugin_instance_env(&self, module: &Module) -> Result<(Store<PluginEnv>, Instance)> {
        let err_context = || {
            format!(
                "Failed to create instance, plugin env and subscriptions for plugin {}",
                self.plugin_id
            )
        };
        let stdin_pipe = Arc::new(Mutex::new(VecDeque::new()));
        let stdout_pipe = Arc::new(Mutex::new(VecDeque::new()));

        let wasi_ctx = PluginLoader::create_wasi_ctx(
            &self.plugin_cwd,
            &self.plugin_own_data_dir,
            &self.plugin_own_cache_dir,
            &ZELLIJ_TMP_DIR,
            self.user_plugin_dir.as_ref(),
            &self.plugin_config.location.to_string(),
            self.plugin_id,
            stdin_pipe.clone(),
            stdout_pipe.clone(),
        )?;
        let plugin_config = self.plugin_config.clone();
        let plugin_env = PluginEnv {
            response_sink: ResponseSink::WasiStdin,
            plugin_id: self.plugin_id,
            cancel: self.cancel.clone(),
            cancel_tripped: Arc::new(AtomicBool::new(false)),
            deno_generation: None,
            deno_workers: None,
            client_id: self.client_id,
            owner_client_id: self.client_id,
            plugin: plugin_config,
            permissions: Arc::new(Mutex::new(None)),
            permissions_requested: Arc::new(AtomicBool::new(false)),
            senders: self.senders.clone(),
            wasi_ctx: Some(wasi_ctx),
            plugin_own_data_dir: self.plugin_own_data_dir.clone(),
            temporary_data_dir: Some(self.temporary_data_dir.clone()),
            data_access: self.data_access.clone(),
            deno_watch_generations: self.deno_watch_generations.clone(),
            queues: self.queues.clone(),
            plugin_own_cache_dir: self.plugin_own_cache_dir.clone(),
            tab_index: self.tab_index,
            path_to_default_shell: self.path_to_default_shell.clone(),
            default_shell: self.default_shell.clone(),
            plugin_cwd: self.plugin_cwd.clone(),
            session_env_vars: self.session_env_vars.clone(),
            plugin_aliases: self.plugin_aliases.clone(),
            input_pipes_to_unblock: Arc::new(Mutex::new(HashSet::new())),
            input_pipes_to_block: Arc::new(Mutex::new(HashSet::new())),
            layout_dir: self.layout_dir.clone(),
            user_plugin_dir: self.user_plugin_dir.clone(),
            // Script runtimes resolve a source file; wasm has its own lookup.
            resolved_source_path: None,
            default_mode: self.default_mode,
            subscriptions: Arc::new(Mutex::new(HashSet::new())),
            keybinds: self.keybinds.clone(),
            intercepting_key_presses: false,
            stdin_pipe,
            stdout_pipe,
            store_limits: PluginResourceLimits::new(self.plugin_id),
        };
        let mut store = Store::new(&self.engine, plugin_env);
        install_cancel_hook(&mut store);

        // See `plugin_limits` for what a plugin is allowed to allocate
        store.limiter(|plugin_env| &mut plugin_env.store_limits);

        let mut linker = Linker::new(&self.engine);
        wasmi_wasi::add_to_linker(&mut linker, |plugin_env: &mut PluginEnv| {
            plugin_env
                .wasi_ctx
                .as_mut()
                .expect("a wasm plugin always has a WASI context")
        })?;
        zellij_exports(&mut linker);

        let instance = linker
            .instantiate_and_start(&mut store, module)
            .with_context(err_context)?;

        if let Some(func) = instance.get_func(&mut store, "_initialize") {
            if let Ok(typed_func) = func.typed::<(), ()>(&store) {
                let _ = typed_func.call(&mut store, ());
            }
        }

        Ok((store, instance))
    }
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::ptr_arg)]
    pub fn create_wasi_ctx(
        host_dir: &PathBuf,
        data_dir: &PathBuf,
        cache_dir: &PathBuf,
        tmp_dir: &PathBuf,
        plugins_dir: Option<&PathBuf>,
        plugin_url: &String,
        plugin_id: PluginId,
        stdin_pipe: Arc<Mutex<VecDeque<u8>>>,
        stdout_pipe: Arc<Mutex<VecDeque<u8>>>,
    ) -> Result<WasiCtx> {
        let _err_context = || "Failed to create wasi_ctx".to_string();
        let mut dirs = vec![
            ("/host".to_owned(), host_dir.clone()),
            ("/data".to_owned(), data_dir.clone()),
            ("/cache".to_owned(), cache_dir.clone()),
            ("/tmp".to_owned(), tmp_dir.clone()),
        ];
        // The user's plugin folder, for a plugin that writes plugins. Mounted like the others
        // and gated, for writes, by `AuthorPlugins` in the host's `Fs*` commands; a wasm plugin
        // reaching it through WASI directly holds whatever WASI grants, which is why the
        // folder is only mounted when it already exists.
        if let Some(plugins_dir) = plugins_dir {
            dirs.push(("/plugins".to_owned(), plugins_dir.clone()));
        }
        let dirs = dirs.into_iter().filter(|(_dir_name, dir)| {
            // note that this does not protect against TOCTOU errors
            // eg. if one or more of these folders existed at the time of check but was deleted
            // before we mounted in in the wasi environment, we'll crash
            // when we move to a new wasi environment, we should address this with locking if
            // there's no built-in solution
            dir.try_exists().ok().unwrap_or(false)
        });

        // Session environment variables are exposed only through the permission-gated
        // GetSessionEnvironmentVariables host command. Inheriting them into WASI would let a
        // plugin bypass that permission through std::env.
        let mut builder = WasiCtxBuilder::new();

        // Mount directories using the builder
        for (guest_path, host_path) in dirs {
            match open_dir(&host_path) {
                Ok(dir_file) => {
                    let dir = Dir::from_std_file(dir_file);
                    builder.preopened_dir(dir, guest_path)?;
                },
                Err(e) => {
                    log::warn!("Failed to mount directory {:?}: {}", host_path, e);
                },
            }
        }

        let ctx = builder.build();

        // Set up custom stdin/stdout/stderr
        ctx.set_stdin(Box::new(ReadPipe::new(VecDequeInputStream(
            stdin_pipe.clone(),
        ))));
        ctx.set_stdout(Box::new(WritePipe::new(WriteOutputStream(
            stdout_pipe.clone(),
        ))));
        ctx.set_stderr(Box::new(WritePipe::new(WriteOutputStream(Arc::new(
            Mutex::new(LoggingPipe::new(plugin_url, plugin_id)),
        )))));

        Ok(ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::{plugin_cache_key, PluginLoader};
    use std::collections::VecDeque;
    use std::path::Path;
    use std::sync::{Arc, Mutex};
    use zellij_config::plugins::PluginConfig;
    use zellij_types::data::PluginTag;
    use {zellij_types::input::layout::RunPlugin, zellij_types::input::layout::RunPluginLocation};

    #[test]
    fn builtin_plugin_cache_key_includes_only_its_configured_source_directory() {
        let builtin = PluginConfig::from_run_plugin(&RunPlugin {
            location: RunPluginLocation::Zellij(PluginTag::new("tab-bar")),
            ..Default::default()
        })
        .unwrap();
        let external = PluginConfig::from_run_plugin(&RunPlugin {
            location: RunPluginLocation::File("external.wasm".into()),
            ..Default::default()
        })
        .unwrap();

        assert_ne!(
            plugin_cache_key(&builtin, Some(Path::new("one"))),
            plugin_cache_key(&builtin, Some(Path::new("two")))
        );
        assert_eq!(
            plugin_cache_key(&external, Some(Path::new("one"))),
            plugin_cache_key(&external, Some(Path::new("two")))
        );
    }

    #[test]
    fn wasi_context_does_not_inherit_the_server_environment() {
        let dir = tempfile::tempdir().unwrap();
        let plugin_url = "fixture-plugin".to_owned();
        let ctx = PluginLoader::create_wasi_ctx(
            &dir.path().to_path_buf(),
            &dir.path().to_path_buf(),
            &dir.path().to_path_buf(),
            &dir.path().to_path_buf(),
            None,
            &plugin_url,
            zellij_types::internal_ids::PluginId::new(1),
            Arc::new(Mutex::new(VecDeque::new())),
            Arc::new(Mutex::new(VecDeque::new())),
        )
        .unwrap();

        assert_eq!(ctx.env.number_elements(), 0);
    }
}
