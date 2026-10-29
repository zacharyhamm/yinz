//! Plugin preparation, instance replacement, client attachment, and teardown.
use super::*;

fn make_plugin_url_path_safe(url: String) -> String {
    url
}

#[derive(Debug, Clone)]
pub struct LoadingContext {
    pub(crate) panes: PluginPanes,
    /// Set for a load or a reload: its prepare step reads the plugin's pane declaration. `None`
    /// for a new client's instance of a plugin already running, which keeps what it has.
    pub(crate) pane_ticket: Option<PaneTicket>,
    /// The entry source the prepare step read, which the loader evaluates, with the pane
    /// declaration the step scanned in it: the loader checks the evaluated module against the
    /// scan of the very source it evaluated.
    pub(crate) prefetched_source: Option<(Option<PathBuf>, Arc<str>)>,
    pub(crate) prefetched_declaration: Option<bool>,
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
    pub default_mode: InputMode,
    pub keybinds: Keybinds,
    pub(crate) configuration_version: u64,
    pub(crate) pending_configuration: Arc<crate::plugin_map::PendingConfiguration>,
    pub yinz_data_dir: PathBuf,
    pub data_access: Arc<DataAccess>,
    pub watch_generations: crate::plugin_map::WatchGenerations,
    pub size: Size,
    pub cancel: Arc<AtomicBool>,
    pub client_cancel: Arc<AtomicBool>,
    pub client_cancellations: Arc<Mutex<HashMap<ClientId, Arc<AtomicBool>>>>,
}

/// Which load of a plugin a prepare step reads the pane declaration for.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PaneTicket {
    pub(crate) load_id: u64,
    /// A reload: the plugin already has, or runs without, a pane, and the declaration may have
    /// changed since.
    pub(crate) reload: bool,
}

/// How a prepared load that cannot be queued is reported.
#[derive(Debug, Clone, Copy)]
enum LoadFailure {
    /// The plugin failed to load, for every client.
    Plugin,
    /// Only a new client's instance failed: the plugin still runs for the others.
    Attach,
}

impl LoadFailure {
    fn report(
        self,
        senders: &ThreadSenders,
        load_id: u64,
        plugin_id: PluginId,
        error: impl std::fmt::Debug,
        client_id: Option<ClientId>,
    ) {
        match self {
            Self::Plugin => {
                handle_plugin_loading_failure(senders, Some(load_id), plugin_id, error, client_id)
            },
            Self::Attach => {
                log::error!("{:?}", error);
                finish_failed_attach(senders, load_id, plugin_id);
            },
        }
    }
}

impl LoadingContext {
    /// Git runs outside both the executor and plugin map. The queued context pins the revision.
    /// Every failure to queue the load is reported as `failure` says.
    fn execute_prepared<F>(
        mut self,
        executor: Arc<PinnedExecutor>,
        senders: ThreadSenders,
        load_id: u64,
        register: bool,
        failure: LoadFailure,
        job: F,
    ) -> bool
    where
        F: FnOnce(Result<Self>, ThreadSenders, Arc<Mutex<PluginMap>>, Arc<Mutex<Vec<ClientId>>>)
            + Send
            + 'static,
    {
        let plugin_id = self.plugin_id;
        if register {
            executor.register_plugin(plugin_id);
        }
        let needs_repository = matches!(self.plugin_config.location, RunPluginLocation::Git(_))
            && self.plugin_config.repository_root.is_none();
        // The declaration waits for the repository, which can take a while: the screen learns of
        // the load now, so that a second launch finds it, and the launch is answered with the
        // plugin's id now rather than after the clone.
        if needs_repository
            && let Some(PaneTicket {
                load_id: ticket,
                reload: false,
            }) = self.pane_ticket
        {
            let mut panes = self.panes.lock().unwrap();
            let effects = panes.resolving(plugin_id, ticket);
            send_effects(&senders, effects);
        }
        let spawn_senders = senders.clone();
        let enqueue = move || {
            let prepared = (|| {
                if let RunPluginLocation::Git(source) = &self.plugin_config.location
                    && self.plugin_config.repository_root.is_none()
                {
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
                            return Err(error.context(
                                crate::plugin_authoring::LogIdentity::repository(source).marker(),
                            ))
                        },
                    }
                }
                self.declare_pane(&senders)
            })();
            match prepared {
                Ok(false) => return true,
                Err(error) if !self.pane_ticket.is_some_and(|ticket| ticket.reload) => {
                    failure.report(&senders, load_id, plugin_id, error, Some(self.client_id));
                    return true;
                },
                _ => {},
            }
            // A failed preparation still owes the old instance its executor-bound cleanup.
            let context = prepared.map(|_| self);
            let work = move |senders, map, clients| job(context, senders, map, clients);
            let accepted = executor.execute_for_plugin_tagged(plugin_id, "load", false, work);
            if !accepted {
                failure.report(
                    &senders,
                    load_id,
                    plugin_id,
                    "Plugin executor rejected loading",
                    None,
                );
            }
            accepted
        };
        if needs_repository {
            let spawned = std::thread::Builder::new()
                .name(format!("plugin-repository-{plugin_id}"))
                .spawn(enqueue);
            if let Err(error) = &spawned {
                failure.report(
                    &spawn_senders,
                    load_id,
                    plugin_id,
                    format!("Could not start plugin repository thread: {error}"),
                    None,
                );
            }
            spawned.is_ok()
        } else {
            enqueue()
        }
    }

    /// Read the entry's pane declaration and tell the ledger, before the load is queued: the
    /// plugin's pane, if it gets one, then reaches the screen ahead of anything `load` sends,
    /// and the decision never waits on the executor (a guest blocked opening another plugin's
    /// pane must not wait for that plugin's load to be scheduled behind it).
    ///
    /// Answers whether to queue the load; the caller reports preparation failures.
    pub(super) fn declare_pane(&mut self, senders: &ThreadSenders) -> Result<bool> {
        let Some(ticket) = self.pane_ticket else {
            return Ok(true);
        };
        // A Lua entry is refused by the loader with its own message; it has nothing to scan.
        if self
            .plugin_config
            .path
            .extension()
            .and_then(|ext| ext.to_str())
            == Some("lua")
        {
            return Ok(true);
        }
        let scanned = script_source(
            &self.plugin_config,
            self.user_plugin_dir.as_deref(),
            &self.yinz_data_dir,
        )
        .and_then(|(path, source)| {
            let declaration = yinz_steel::manifest::readable_pane_declaration(&source)
                .context("reading the plugin's pane declaration")?;
            Ok((path, source, declaration))
        });
        let (path, source, declaration) = scanned.with_context(|| {
            format!(
                "failed to start the script plugin at '{}'",
                self.plugin_config.path.display()
            )
        })?;
        self.prefetched_source = Some((path, source.into()));
        // Preserve reload slots; answer new launches now so evaluation cannot block their opener.
        let Some(declaration) = declaration else {
            if !ticket.reload {
                let mut panes = self.panes.lock().unwrap();
                let (prepared, effects) = panes.declare(self.plugin_id, ticket.load_id, true);
                send_effects(senders, effects);
                return Ok(prepared != Prepared::Abort);
            }
            return Ok(true);
        };
        let declared =
            declaration.applies(self.plugin_config.initial_userspace_configuration.inner());
        let prepared = {
            let mut panes = self.panes.lock().unwrap();
            let (prepared, effects) = if ticket.reload {
                panes.redeclare(self.plugin_id, ticket.load_id, declared)
            } else {
                panes.declare(self.plugin_id, ticket.load_id, declared)
            };
            send_effects(senders, effects);
            prepared
        };
        if prepared == Prepared::Abort {
            // The ledger's `Unload` effect retires the load.
            return Ok(false);
        }
        self.prefetched_declaration = Some(declared);
        Ok(true)
    }

    pub fn new(
        plugin_bridge: &PluginBridge,
        cwd: Option<PathBuf>,
        plugin_config: PluginConfig,
        plugin_id: PluginId,
        client_id: ClientId,
        tab_index: Option<TabId>,
        size: Size,
    ) -> Result<Self> {
        let temporary_data_dir = plugin_bridge
            .temporary_root
            .child()
            .context("failed to create plugin /data")?;
        let plugin_own_data_dir = temporary_data_dir.path().to_path_buf();
        // Canonical file identity can change on reload; /cache belongs to the existing plugin.
        let plugin_own_cache_dir = plugin_bridge
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
        let configuration = plugin_bridge.plugin_configuration(client_id);
        let plugin_cwd = cwd.unwrap_or_else(|| plugin_bridge.yinz_cwd.clone());
        let data_owner;
        let configuration_version;
        let pending_configuration;
        {
            let mut plugin_map = plugin_bridge.plugin_map.lock().unwrap();
            configuration_version = plugin_map.configuration_version;
            pending_configuration =
                plugin_map.pending_configuration(&plugin_bridge.plugin_map, client_id);
            plugin_map
                .client_configurations
                .entry(client_id)
                .or_insert_with(|| (configuration_version, configuration.clone()));
            let previous = plugin_map.reload_metadata(plugin_id);
            data_owner = previous
                .as_ref()
                .map(|metadata| metadata.data_owner.clone())
                .unwrap_or_else(|| Url::from(&plugin_config.location));
            let resolved_source_path = previous.and_then(|metadata| metadata.resolved_source_path);
            plugin_map.remember_plugin(
                plugin_id,
                crate::plugin_map::PluginMetadata {
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
            panes: plugin_bridge.panes.clone(),
            pane_ticket: None,
            prefetched_source: None,
            prefetched_declaration: None,
            pick_opening: plugin_bridge
                .picks
                .get(&plugin_id)
                .map(|pick| pick.opening.clone()),
            pick_cancelled: plugin_bridge
                .picks
                .get(&plugin_id)
                .map(|pick| pick.cancelled.clone()),
            pick_request: plugin_bridge
                .picks
                .get(&plugin_id)
                .map(|pick| pick.request.clone()),
            client_id,
            plugin_id,
            path_to_default_shell: plugin_bridge.path_to_default_shell.clone(),
            session_env_vars: plugin_bridge.session_env_vars.clone(),
            server_session: plugin_bridge.server_session.clone(),
            plugin_aliases: plugin_bridge.plugin_aliases.clone(),
            plugin_cwd,
            default_shell: configuration.default_shell,
            layout_dir: configuration.layout_dir,
            user_plugin_dir: configuration.user_plugin_dir,
            keybinds: configuration.keybinds,
            default_mode: configuration.default_mode,
            configuration_version,
            pending_configuration,
            plugin_own_data_dir,
            temporary_data_dir,
            plugin_own_cache_dir,
            plugin_config,
            data_owner,
            tab_index,
            yinz_data_dir: plugin_bridge.yinz_data_dir.clone(),
            data_access: plugin_bridge.data_access.clone(),
            watch_generations: plugin_bridge.watch_generations.clone(),
            size,
            // Made by the load or reload this context is for; a context built for a plugin
            // nothing registered (an `add_client` for one mid-unload) gets a token nobody sets.
            cancel: plugin_bridge
                .cancel_tokens
                .get(&plugin_id)
                .cloned()
                .unwrap_or_default(),
            client_cancel: client_cancel_token(&plugin_bridge.client_cancellations, client_id),
            client_cancellations: plugin_bridge.client_cancellations.clone(),
        })
    }
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
                            rp.env().generation,
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

/// The attachment token for work done for `client_id`. A client registered by
/// [`PluginBridge::reserve_client`] shares its token, which its `RemoveClient` cancels. Any other
/// id gets a token of its own that is never cancelled, and nothing is registered for it. This
/// covers a departed client, a CLI connection, and [`ClientId::NONE`]. Registering it would
/// leave an entry that no `RemoveClient` comes back for, and its work runs as a late
/// `launch-plugin` must.
pub(crate) fn client_cancel_token(
    client_cancellations: &Mutex<HashMap<ClientId, Arc<AtomicBool>>>,
    client_id: ClientId,
) -> Arc<AtomicBool> {
    client_cancellations
        .lock()
        .unwrap()
        .get(&client_id)
        .cloned()
        .unwrap_or_default()
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
        let connected = connected_clients.lock().unwrap().clone();
        plugin_map.remove_disconnected_instances(plugin_id, &connected)
    };
    for (client_id, (plugin, subscriptions)) in removed {
        close_plugin_instance(plugin_id, client_id, plugin, subscriptions, senders.clone());
    }
}

impl Drop for PluginBridge {
    fn drop(&mut self) {
        self.cancel_all_plugins();
    }
}

impl PluginBridge {
    pub(super) fn begin_plugin_load(&mut self, plugin_id: PluginId) -> u64 {
        self.latest_plugin_errors.remove(&plugin_id);
        self.next_load_id += 1;
        self.load_ids.insert(plugin_id, self.next_load_id);
        self.next_load_id
    }

    /// The load a client attaching to a running plugin loads its instance under: the plugin's
    /// current one. An attach adds an instance and replaces nothing, so it must not supersede a
    /// load still in flight (a reload, or a git reload still resolving its repository): that
    /// load's failure, cached pipes and cached events would all be dropped as stale.
    fn join_plugin_load(&mut self, plugin_id: PluginId) -> u64 {
        match self.load_ids.get(&plugin_id) {
            Some(load_id) => {
                self.latest_plugin_errors.remove(&plugin_id);
                *load_id
            },
            None => self.begin_plugin_load(plugin_id),
        }
    }

    pub(crate) fn load_is_current(&self, plugin_id: PluginId, load_id: u64) -> bool {
        self.load_ids.get(&plugin_id) == Some(&load_id)
    }

    /// Load `run` as a new plugin. `pane` says what its launcher asked for; the plugin's own
    /// declaration decides, in the prepare step, whether it gets that pane or runs headless (see
    /// `crate::plugin_panes`). Callers never place the pane themselves.
    pub fn load_plugin(
        &mut self,
        run: &Option<RunPlugin>,
        tab_index: Option<TabId>,
        size: Size,
        cwd: Option<PathBuf>,
        _skip_cache: bool,
        client_id: Option<ClientId>,
        pane: PaneRequest,
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
            });
        let client_id = match client_id {
            Ok(client_id) => client_id,
            Err(error) => {
                let location = run.as_ref().map_or_else(
                    || "an unresolved plugin".to_owned(),
                    |run| run.location.to_string(),
                );
                let error = error.context(format!("failed to start {location}"));
                pane.reject(&format!("{error:#}"));
                return Err(error);
            },
        };
        let pane = pane.for_client(client_id);

        let plugin_id = PluginId::new(self.next_plugin_id);
        // Taken now, whatever becomes of this load: an id is never reused (the screen keeps
        // unloaded ids to drop late pane records for them), including one whose load fails
        // before it is queued.
        self.next_plugin_id += 1;
        // A load that fails before it is prepared is never declared; its id is unused.
        let unprepared_load = 0;
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
                        self.panes
                            .lock()
                            .unwrap()
                            .request(plugin_id, unprepared_load, pane);
                        handle_plugin_loading_failure(
                            &self.senders,
                            None,
                            plugin_id,
                            format!("Failed to resolve plugin: {}", run.location),
                            Some(client_id),
                        );
                        return Ok((plugin_id, client_id));
                    },
                };

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
                // Recorded before the load is prepared, which is where the ledger learns the
                // declaration and answers it.
                self.panes.lock().unwrap().request(plugin_id, load_id, pane);
                let mut loading_context = match LoadingContext::new(
                    self,
                    Some(yinz_cwd.clone()),
                    plugin_config,
                    plugin_id,
                    client_id,
                    tab_index,
                    size,
                ) {
                    Ok(loading_context) => loading_context,
                    Err(error) => {
                        // Failed as a rejected load is, as a reload whose preparation fails is:
                        // the plugin leaves `loading_plugins` (or pipes and reloads by URL would
                        // keep finding a load that never runs), its cached pipes are rejected,
                        // its status shows why, and the ledger answers the request.
                        self.plugin_failed(
                            plugin_id,
                            Some(load_id),
                            None,
                            None,
                            format!("{error:#}"),
                            true,
                        )
                        .with_context(|| format!("fail plugin {plugin_id}, which was not prepared"))
                        .non_fatal();
                        return Err(error);
                    },
                };
                loading_context.pane_ticket = Some(PaneTicket {
                    load_id,
                    reload: false,
                });

                let accepted = loading_context.execute_prepared(
                    self.plugin_executor.clone(),
                    senders.clone(),
                    load_id,
                    true,
                    LoadFailure::Plugin,
                    move |loading_context, senders, plugin_map, connected_clients| {
                        if load_was_cancelled(
                            &unloaded_while_loading,
                            plugin_id,
                            &senders,
                            &plugin_executor,
                        ) {
                            return;
                        }
                        let loaded = loading_context.and_then(|loading_context| {
                            PluginLoader::new(
                                loading_context,
                                senders.clone(),
                                plugin_map.clone(),
                                connected_clients.clone(),
                            )
                            .start_plugin()
                        });
                        if load_was_cancelled(
                            &unloaded_while_loading,
                            plugin_id,
                            &senders,
                            &plugin_executor,
                        ) {
                            return;
                        }
                        match loaded {
                            Ok(_) => {
                                let plugin_list = plugin_map.lock().unwrap().list_plugins();
                                handle_plugin_successful_loading(&senders, plugin_id, plugin_list);
                            },
                            Err(e) => handle_plugin_loading_failure(
                                &senders,
                                Some(load_id),
                                plugin_id,
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
                        "Plugin executor rejected loading",
                        None,
                    );
                }
            },
            None => {
                self.panes
                    .lock()
                    .unwrap()
                    .request(plugin_id, unprepared_load, pane);
                handle_plugin_loading_failure(
                    &self.senders,
                    None,
                    plugin_id,
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
    pub(super) fn cancel_token(&mut self, plugin_id: PluginId) -> Arc<AtomicBool> {
        let cancel = self.cancel_tokens.entry(plugin_id).or_default().clone();
        self.plugin_map
            .lock()
            .unwrap()
            .set_load_token(plugin_id, cancel.clone());
        cancel
    }

    /// `unload_plugin`, with a bound on how long to wait for `plugin_map` when one is given.
    fn unload_plugin_with_deadline(
        &mut self,
        pid: PluginId,
        plugin_map_deadline: Option<Instant>,
    ) -> Result<()> {
        self.pane_hooks.retain(|_, plugin_id| *plugin_id != pid);
        self.repl_pipe_owners.remove(&pid);
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
        // Cancel a pending load before removing its identity so it cannot publish an instance.
        let still_loading = self.loading_plugins.iter().any(|(p_id, _)| *p_id == pid);
        if let (Some(cancel), true) = (&cancel, still_loading) {
            cancel.store(true, Ordering::Relaxed);
        }
        let _ = self.senders.send_to_server(ServerInstruction::RebindKeys {
            client_id: ClientId::NONE,
            keys_to_rebind: vec![],
            keys_to_unbind: vec![],
            write_config_to_disk: false,
            plugin_id: Some(pid),
        });
        self.cancel_permission_requests(pid, None);
        {
            let mut panes = self.panes.lock().unwrap();
            let effects = panes.retire(pid, shutting_down);
            send_effects(&self.senders, effects);
        }
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
        if plugins_to_cleanup.is_empty() && still_loading {
            let _ = self
                .senders
                .send_to_background_jobs(BackgroundJob::StopPluginLoadingAnimation(pid));
            let unloaded_while_loading = self.unloaded_while_loading.clone();
            self.plugin_executor
                .execute_plugin_unload(pid, move |_, _, _| {
                    unloaded_while_loading.lock().unwrap().remove(&pid);
                });
        }
        if let (Some(cancel), false, false) =
            (&cancel, shutting_down, plugins_to_cleanup.is_empty())
        {
            let instances = plugins_to_cleanup
                .iter()
                .map(|(_, (running_plugin, _))| running_plugin.clone())
                .collect();
            arm_unload_watchdog(pid, cancel.clone(), instances);
        }
        for (index, ((plugin_id, client_id), (running_plugin, subscriptions))) in
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

            let cleanup =
                move |senders: ThreadSenders,
                      _plugin_map: Arc<Mutex<PluginMap>>,
                      _connected_clients: Arc<Mutex<Vec<ClientId>>>| {
                    close_plugin_instance(
                        plugin_id,
                        client_id,
                        running_plugin,
                        subscriptions,
                        senders,
                    );
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

        Ok(())
    }

    /// A plugin failed to load or trapped. Retire failed instances while keeping their
    /// metadata and error pane available for explicit reload.
    pub fn plugin_failed(
        &mut self,
        plugin_id: PluginId,
        load_id: Option<u64>,
        generation: Option<u64>,
        instance_id: Option<PluginInstanceId>,
        error: String,
        failed_to_load: bool,
    ) -> Result<()> {
        if load_id.is_some_and(|id| !self.load_is_current(plugin_id, id)) {
            return Ok(());
        }
        if !failed_to_load {
            let mut map = self.plugin_map.lock().unwrap();
            let instances = map.pipe_failure_instances(plugin_id, instance_id, generation);
            if instances.is_empty() && (instance_id.is_some() || generation.is_some()) {
                return Ok(());
            }
            // The failed instances were just marked unavailable; another client's may run on.
            let stopped = !map.has_available_instance(plugin_id);
            drop(map);
            let mut panes = self.panes.lock().unwrap();
            let effects =
                panes.failure_effects(plugin_id, &error, Failure::Crash { generation }, stopped);
            send_effects(&self.senders, effects);
            drop(panes);
            for (id, instance) in instances {
                let accepted = self.plugin_executor.execute_for_plugin_tagged(
                    plugin_id,
                    "failed_dispose",
                    false,
                    move |_, _, _| instance.lock().unwrap().dispose(),
                );
                if !accepted {
                    self.pending_pipes.retire(plugin_id, id, &error);
                }
            }
        } else {
            self.plugin_map
                .lock()
                .unwrap()
                .reload_data_dirs
                .remove(&plugin_id);
            // A pane shows the failure; a headless plugin launched on request toasts once per
            // load (each load has its own id), and one started in the background is reported
            // only by its status and the log.
            let mut panes = self.panes.lock().unwrap();
            let effects = panes.failure_effects(plugin_id, &error, Failure::Load { load_id }, true);
            // The ledger stops the animation where it paints the failure; any other load may
            // have started one too.
            let painted = effects
                .iter()
                .any(|effect| matches!(effect, PaneEffect::Background(_)));
            send_effects(&self.senders, effects);
            if !painted {
                self.senders
                    .send_to_background_jobs(BackgroundJob::StopPluginLoadingAnimation(plugin_id))
                    .context("stop the loading animation of a plugin that failed to load")
                    .non_fatal();
            }
            drop(panes);
            self.pending_pipes.reject_cached(plugin_id, load_id, &error);
        }
        self.cancel_plugin_picks(plugin_id);
        if !failed_to_load && !self.loading_plugins.iter().any(|(id, _)| *id == plugin_id) {
            let instances = self.plugin_map.lock().unwrap().pipe_instances();
            let shared = self.plugin_is_shared(&plugin_id);
            if let Some(cache) = self.cached_events_for_pending_plugins.get_mut(&plugin_id) {
                cache.retain(|message| {
                    if let EventOrPipeMessage::PipeMessage(client, _, request, load) = message
                        && !instances.iter().any(|((id, owner, _, _), _)| {
                            Self::message_is_directed_at_plugin(
                                Some(plugin_id),
                                *client,
                                id,
                                owner,
                                shared,
                            )
                        })
                    {
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
        self.latest_plugin_errors.insert(plugin_id, error);
        self.finish_pipe_requests();

        if failed_to_load {
            let run_plugin = self.run_plugin_of_loading_plugin_id(plugin_id).cloned();
            self.cached_events_for_pending_plugins.remove(&plugin_id);
            self.cached_resizes_for_pending_plugins.remove(&plugin_id);
            self.loading_plugins.retain(|(id, _)| *id != plugin_id);
            self.plugin_ids_waiting_for_permission_request
                .remove(&plugin_id);
            if self.pending_plugin_id_reloads.remove(&plugin_id) {
                self.reload_plugin_with_id(plugin_id)
                    .with_context(|| format!("run deferred reload of plugin {plugin_id}"))
                    .non_fatal();
            }
            if let Some(run_plugin) = run_plugin
                && self.pending_plugin_reloads.remove(&run_plugin)
            {
                self.reload_plugin(&run_plugin)
                    .with_context(|| format!("run deferred reload of {}", run_plugin.location))
                    .non_fatal();
            }
        }
        Ok(())
    }

    pub fn reload_plugin_with_id(&mut self, plugin_id: PluginId) -> Result<()> {
        let picker = self.picks.contains_key(&plugin_id);
        self.cancel_plugin_picks(plugin_id);
        if picker {
            return Ok(());
        }
        // Looked up before the old instances are retired or the plugin is marked as loading,
        // so a plugin that cannot be found keeps running rather than being left retired, or
        // loading with its events cached forever.
        let Some(run_plugin) = self.run_plugin_of_plugin_id(plugin_id) else {
            log::error!("Failed to find plugin with id: {}", plugin_id);
            return Ok(());
        };
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
        let Some((rows, columns)) = self.size_of_plugin_id(plugin_id) else {
            log::error!(
                "Could not find size of running plugin with id: {}",
                plugin_id
            );
            return Ok(());
        };
        let size = Size {
            rows,
            cols: columns,
        };
        let cwd = self.cwd_of_plugin_id(plugin_id);

        // Retire queued callbacks before the reload notification can release an active host call.
        let cancel = self.cancel_token(plugin_id);
        {
            let mut map = self.plugin_map.lock().unwrap();
            map.retire_instances(plugin_id);
            // Supersede initializers atomically with retirement, without cancelling guest cleanup.
            let replacement = Arc::new(AtomicBool::new(false));
            map.set_load_token(plugin_id, replacement.clone());
            self.cancel_tokens.insert(plugin_id, replacement);
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
        let reload_pending = {
            let instances = self
                .plugin_map
                .lock()
                .unwrap()
                .running_plugins_and_subscriptions()
                .into_iter()
                .filter(|(id, ..)| *id == plugin_id)
                .map(|(_, _, running_plugin, _)| Arc::downgrade(&running_plugin))
                .collect();
            Some(arm_reload_watchdog(plugin_id, cancel, instances))
        };

        // A failure already queued by the old generation must not
        // paint over this reload, even before the executor installs the replacement.
        self.plugin_map
            .lock()
            .unwrap()
            .invalidate_generations(plugin_id);

        self.plugin_ids_waiting_for_permission_request
            .remove(&plugin_id);
        self.pending_pipes
            .reject_cached(plugin_id, None, "Plugin reloaded before pipe delivery");
        self.cached_events_for_pending_plugins.remove(&plugin_id);
        self.finish_pipe_requests();
        self.cached_events_for_pending_plugins
            .entry(plugin_id)
            .or_default();
        self.cached_resizes_for_pending_plugins
            .insert(plugin_id, (rows, columns));

        // A headless plugin has no pane to show its reload in.
        if self.panes.lock().unwrap().surface(plugin_id) != Some(Surface::Headless) {
            let loading_indication = LoadingIndication::new(run_plugin.location.to_string());
            self.start_plugin_loading_indication(&[plugin_id], &loading_indication);
        }
        self.loading_plugins.insert((plugin_id, run_plugin.clone()));

        let plugin_executor = self.plugin_executor.clone();

        let load_id = self.begin_plugin_load(plugin_id);
        // Only this reload's declaration decides the plugin's pane from now on.
        self.panes.lock().unwrap().reload(plugin_id, load_id);
        let loading_context = LoadingContext::new(
            self,
            cwd,
            plugin_config,
            plugin_id,
            first_client_id,
            tab_index,
            size,
        );
        self.plugin_map
            .lock()
            .unwrap()
            .reload_data_dirs
            .entry(plugin_id)
            .or_default();

        let cancel = self.cancel_token(plugin_id);
        let reload = move |mut loading_context: Result<LoadingContext>,
                           senders: ThreadSenders,
                           plugin_map: Arc<Mutex<PluginMap>>,
                           connected_clients: Arc<Mutex<Vec<ClientId>>>| {
            let instances = {
                let mut map = plugin_map.lock().unwrap();
                // A failed instance retains its identity; explicit unload removes it.
                if !map.load_is_current(plugin_id, &cancel)
                    || map.run_plugin_of_plugin_id(plugin_id).is_none()
                {
                    return;
                }
                if let Some(pending) = reload_pending {
                    // The old call returned; its watchdog must not interrupt BeforeClose.
                    *pending.lock().unwrap() = false;
                }
                map.running_plugins_and_subscriptions()
            };
            // The instance about to be replaced gets the same last word an unloaded one
            // does, so a plugin that persists in `BeforeClose` survives its own reload.
            // Cleanup can synchronously query metadata through the plugin thread.
            for (id, client_id, running_plugin, subscriptions) in instances {
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
                        std::process::id(), running_plugin.env().generation,
                        running_plugin.is_disposed(), running_plugin.failed,
                        running_plugin.env().cancel.load(Ordering::Acquire),
                        running_plugin.env().cancel_tripped.load(Ordering::Acquire),
                    );
                let retained_directory = if client_id == first_client_id
                    || running_plugin.env().plugin_config.is_shared()
                {
                    running_plugin.env().temporary_data_dir.clone()
                } else {
                    None
                };
                if wants_before_close && !running_plugin.is_disposed() && !running_plugin.failed {
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
                        if running_plugin.env().generation.is_some() {
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
                let mut map = plugin_map.lock().unwrap();
                // A failed or unloaded replacement must not regain ownership after cleanup.
                if let (Some(retained), Some(directory)) =
                    (map.reload_data_dirs.get_mut(&plugin_id), retained_directory)
                {
                    *retained = Some(directory);
                }
                map.remove_instance(plugin_id, client_id);
            }
            {
                let map = plugin_map.lock().unwrap();
                if !map.load_is_current(plugin_id, &cancel) {
                    return;
                }
                if let Ok(loading_context) = &mut loading_context
                    && let Some(Some(directory)) = map.reload_data_dirs.get(&plugin_id)
                {
                    loading_context.plugin_own_data_dir = directory.path().to_path_buf();
                    loading_context.temporary_data_dir = directory.clone();
                }
            }
            // A replacement must opt in again, including when BeforeClose resubscribed.
            let _ =
                senders.send_to_screen(ScreenInstruction::UnsubscribePaneOutput(plugin_id, None));
            let loaded = loading_context.and_then(|loading_context| {
                PluginLoader::new(
                    loading_context,
                    senders.clone(),
                    plugin_map.clone(),
                    connected_clients.clone(),
                )
                .start_plugin()
            });
            let mut plugin_map = plugin_map.lock().unwrap();
            if !plugin_map.load_is_current(plugin_id, &cancel) {
                return;
            }
            plugin_map.reload_data_dirs.remove(&plugin_id);
            match loaded {
                Ok(_) => {
                    if let Some(location) = plugin_map.update_reloaded_file_location(plugin_id) {
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
                    e,
                    Some(first_client_id),
                ),
            }
            let _ = senders.send_to_plugin(PluginInstruction::ApplyCachedEvents {
                load_id,
                plugin_ids: vec![plugin_id],
                done_receiving_permissions: false,
            });
        };
        let accepted = match loading_context {
            Ok(mut context) => {
                // The declaration is read again: the reload may have added or dropped it.
                context.pane_ticket = Some(PaneTicket {
                    load_id,
                    reload: true,
                });
                context.execute_prepared(
                    plugin_executor,
                    self.senders.clone(),
                    load_id,
                    false,
                    LoadFailure::Plugin,
                    reload,
                )
            },
            Err(error) => plugin_executor.execute_for_plugin_tagged(
                plugin_id,
                "reload cleanup",
                false,
                move |senders, map, clients| reload(Err(error), senders, map, clients),
            ),
        };
        if !accepted {
            self.fail_replacement(
                plugin_id,
                load_id,
                "Plugin executor rejected replacement",
                "Plugin executor rejected loading".into(),
            )?;
        }
        Ok(())
    }
    /// Fail a reload whose old instances are already retired: pipes waiting on the plugin
    /// hear `pipe_reason`, and the plugin leaves `loading_plugins` showing `message`.
    fn fail_replacement(
        &mut self,
        plugin_id: PluginId,
        load_id: u64,
        pipe_reason: &str,
        message: String,
    ) -> Result<()> {
        self.pending_pipes.retire_plugin(plugin_id, pipe_reason);
        self.plugin_failed(plugin_id, Some(load_id), None, None, message, true)
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
        // One instance that fails to reload must not leave the others on the old code.
        let mut result = Ok(());
        for plugin_id in &plugin_ids {
            if let Err(error) = self
                .reload_plugin_with_id(*plugin_id)
                .with_context(|| format!("reload plugin {plugin_id}"))
            {
                if result.is_ok() {
                    result = Err(error);
                } else {
                    Err::<(), _>(error).non_fatal();
                }
            }
        }
        result
    }
    pub fn add_client(&mut self, client_id: ClientId) -> Result<()> {
        let started_at = std::time::Instant::now();
        log::info!(
            "attach trace: PluginBridge::add_client client_id={client_id} connected-client check begin"
        );
        if self.client_is_connected(&client_id) {
            log::info!(
                "attach trace: PluginBridge::add_client client_id={client_id} already connected"
            );
            return Ok(());
        }

        // Registered before the client is listed as connected; see `remove_client`.
        let client_cancel = self.reserve_client(client_id);
        // Its own keybinds and base mode from here on, starting from the session-wide ones it has
        // been running with, so that another client's reconfigure moving those leaves it alone.
        // A `Reconfigure` for it that came first has already set them.
        self.keybinds
            .entry(client_id)
            .or_insert_with(|| self.default_keybinds.clone());
        self.base_modes
            .entry(client_id)
            .or_insert(self.default_mode);
        self.connected_clients.lock().unwrap().push(client_id);
        {
            let mut map = self.plugin_map.lock().unwrap();
            let version = map.configuration_version;
            map.client_configurations
                .insert(client_id, (version, self.plugin_configuration(client_id)));
        }
        let mut new_plugins = HashSet::new();
        log::info!(
            "attach trace: PluginBridge::add_client client_id={client_id} waiting for plugin_map lock"
        );
        let existing_plugin_ids = self.plugin_map.lock().unwrap().plugin_ids();
        log::info!(
            "attach trace: PluginBridge::add_client client_id={client_id} plugin_map read complete plugins={} after {:?}",
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
                "attach trace: PluginBridge::add_client client_id={client_id} plugin_id={plugin_id} instance check begin"
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
                    "attach trace: PluginBridge::add_client client_id={client_id} plugin_id={plugin_id} already instantiated"
                );
                continue;
            }
            let Some(run_plugin) = self.run_plugin_of_plugin_id(plugin_id) else {
                log::error!("Failed to find plugin with id: {}", plugin_id);
                continue;
            };
            // A plugin whose declaration is not known yet (a git plugin still resolving its
            // repository) has no surface for this client's instance to take. Its load has not
            // been queued either, since the declaration is read first, so that load creates
            // this client's instance with the others once it runs, with the pane or without
            // it as the declaration says. One that failed before its declaration was known
            // runs for nobody until it is reloaded, which then does the same.
            if self.panes.lock().unwrap().pending(plugin_id) {
                log::info!(
                    "attach trace: PluginBridge::add_client client_id={client_id} plugin_id={plugin_id} left to its pending load"
                );
                continue;
            }

            // Looked up before the plugin is marked as loading, so a plugin that cannot be
            // found is skipped without being left loading.
            let Some(plugin_config) = self.plugin_config_of_plugin_id(plugin_id) else {
                log::error!("Could not find running plugin with id: {}", plugin_id);
                continue;
            };
            let tab_index = self.tab_index_of_plugin_id(plugin_id);
            let Some((rows, columns)) = self.size_of_plugin_id(plugin_id) else {
                log::error!(
                    "Could not find size of running plugin with id: {}",
                    plugin_id
                );
                continue;
            };
            let size = Size {
                rows,
                cols: columns,
            };
            let cwd = self.cwd_of_plugin_id(plugin_id);

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

            // A headless plugin has no pane to show this client's instance loading in.
            if self.panes.lock().unwrap().surface(plugin_id) != Some(Surface::Headless) {
                let loading_indication = LoadingIndication::new(run_plugin.location.to_string());
                self.start_plugin_loading_indication(&[plugin_id], &loading_indication);
            }
            self.loading_plugins.insert((plugin_id, run_plugin.clone()));

            let plugin_executor = self.plugin_executor.clone();

            // So the context below shares the plugin's token rather than getting one nobody
            // sets; a no-op when the plugin is registered, as it normally is here.
            self.cancel_token(plugin_id);
            let load_id = self.join_plugin_load(plugin_id);
            let mut loading_context = match LoadingContext::new(
                self,
                cwd,
                plugin_config,
                plugin_id,
                client_id,
                tab_index,
                size,
            ) {
                Ok(loading_context) => loading_context,
                Err(error) => {
                    // One plugin that cannot load for this client must not fail the attach,
                    // nor the plugin itself: it is still running for the other clients. So this
                    // ends like a load that failed on the executor for a new client, which
                    // replays the cached events to the instances that exist, rather than
                    // through `plugin_failed`, which fails the plugin session-wide.
                    Err::<(), _>(error)
                        .with_context(|| {
                            format!("prepare plugin {plugin_id} for new client {client_id}")
                        })
                        .non_fatal();
                    finish_failed_attach(&self.senders, load_id, plugin_id);
                    continue;
                },
            };
            // This attachment is not connected yet, but must share the token remove_client sets.
            loading_context.client_cancel = client_cancel.clone();

            log::info!(
                "attach trace: PluginBridge::add_client client_id={client_id} plugin_id={plugin_id} executor enqueue begin"
            );
            let accepted = loading_context.execute_prepared(
                plugin_executor, self.senders.clone(), load_id, false, LoadFailure::Attach,
                move |loading_context, senders, plugin_map, connected_clients| {
                    let load_started_at = std::time::Instant::now();
                    log::info!(
                        "attach trace: plugin executor client_id={client_id} plugin_id={plugin_id} waiting for plugin_map lock"
                    );
                    if !plugin_map.lock().unwrap().plugin_ids().contains(&plugin_id) { return; }
                    log::info!(
                        "attach trace: plugin executor client_id={client_id} plugin_id={plugin_id} acquired plugin_map lock after {:?}; plugin load begin",
                        load_started_at.elapsed()
                    );
                    match loading_context.and_then(|loading_context| PluginLoader::new(
                        loading_context,
                        senders.clone(),
                        plugin_map.clone(),
                        connected_clients.clone(),
                    )
                    .without_connected_clients()
                    .start_plugin())
                    {
                        Ok(_) => {
                            log::info!(
                                "attach trace: plugin executor client_id={client_id} plugin_id={plugin_id} plugin load complete after {:?}",
                                load_started_at.elapsed()
                            );
                            // Cancellation can succeed without publishing an instance.
                            if plugin_map.lock().unwrap().has_available_instance(plugin_id) {
                                let _ = senders
                                    .send_to_screen(ScreenInstruction::PluginRecovered(plugin_id));
                            }
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
                            finish_failed_attach(&senders, load_id, plugin_id);
                        },
                    }
                    cleanup_disconnected_instances(plugin_id, &connected_clients, &plugin_map, senders);

                },
            );
            // A rejection was already reported, as ending only this client's load: the plugin is
            // still running for the other clients, so it is not failed session-wide.
            log::info!(
                "attach trace: PluginBridge::add_client client_id={client_id} plugin_id={plugin_id} executor enqueue accepted={accepted} complete after {:?}",
                started_at.elapsed()
            );
        }
        log::info!(
            "attach trace: PluginBridge::add_client client_id={client_id} complete after {:?}",
            started_at.elapsed()
        );
        Ok(())
    }

    pub fn remove_client(&mut self, client_id: ClientId) {
        self.cancel_client_picks(client_id);
        // Remove the client before its token; loaders with an older snapshot skip missing tokens.
        let was_connected = {
            let mut clients = self.connected_clients.lock().unwrap();
            let previous_len = clients.len();
            clients.retain(|client| *client != client_id);
            clients.len() != previous_len
        };
        // Client ids are never reused, so nothing that comes later needs this client's state. A
        // load handled later for it gets a token of its own (`client_cancel_token`), and its
        // plugins run with the session-wide keybinds and base mode.
        if let Some(cancel) = self.client_cancellations.lock().unwrap().remove(&client_id) {
            cancel.store(true, Ordering::Release);
        }
        self.base_modes.remove(&client_id);
        self.keybinds.remove(&client_id);
        {
            let mut map = self.plugin_map.lock().unwrap();
            map.prune_client_configuration(client_id);
        }
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

        // Only a connected client's departure can orphan instances that a connected client must
        // replace. A CLI connection owns instances only while nobody is attached, and the next
        // client to attach retires them. Do not enqueue work for every plugin on each CLI
        // disconnect (agent hooks and session probes are frequent).
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
                    move |senders, plugin_map, connected_clients| {
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
        if let Some(done) = done
            && let Err(std::sync::mpsc::RecvTimeoutError::Timeout) =
                done.recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            log::warn!("plugin temporary data still in use at the shutdown deadline");
        }
    }

    pub fn client_is_connected(&self, client_id: &ClientId) -> bool {
        self.connected_clients.lock().unwrap().contains(client_id)
    }
    /// Register `client_id`'s attachment token, which its `RemoveClient` cancels and forgets,
    /// before anything can be loaded for it: the session's first client at startup, and each
    /// later one from `ReserveClient`. Returns the token.
    pub fn reserve_client(&self, client_id: ClientId) -> Arc<AtomicBool> {
        self.client_cancellations
            .lock()
            .unwrap()
            .entry(client_id)
            .or_default()
            .clone()
    }
}

/// Retire a load unloaded before registration; its cancel token prevents publication.
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

/// End a new client's load of a plugin that still runs for the other clients: the loading
/// animation stops and the cached events replay to the instances that exist.
fn finish_failed_attach(senders: &ThreadSenders, load_id: u64, plugin_id: PluginId) {
    let _ = senders.send_to_background_jobs(BackgroundJob::StopPluginLoadingAnimation(plugin_id));
    let _ = senders.send_to_plugin(PluginInstruction::ApplyCachedEvents {
        load_id,
        plugin_ids: vec![plugin_id],
        done_receiving_permissions: false,
    });
}

pub(super) fn handle_plugin_loading_failure(
    senders: &ThreadSenders,
    load_id: Option<u64>,
    plugin_id: PluginId,
    error: impl std::fmt::Debug,
    client_id: Option<ClientId>,
) {
    log::error!("{:?}", error);
    let _ = senders.send_to_plugin(PluginInstruction::PluginFailed {
        load_id,
        plugin_id,
        generation: None,
        instance_id: None,
        loading_client: client_id,
        error: format!("{:?}", error),
        failed_to_load: true,
    });
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
pub(super) fn arm_unload_watchdog(
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
/// reload job already dropped is not running. The returned gate retires the old call's
/// watchdog before cleanup; its lock orders retirement against cancellation.
fn arm_reload_watchdog(
    plugin_id: PluginId,
    cancel: Arc<AtomicBool>,
    instances: Vec<Weak<Mutex<RunningPlugin>>>,
) -> Arc<Mutex<bool>> {
    let pending = Arc::new(Mutex::new(true));
    let still_pending = pending.clone();
    yinz_bus::global_async_runtime::get_tokio_runtime().spawn(async move {
        tokio::time::sleep(UNLOAD_GRACE).await;
        let pending = still_pending.lock().unwrap();
        if *pending
            && instances
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
    pending
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
    generation: Option<u64>,
    instance_id: Option<PluginInstanceId>,
    message: String,
    senders: ThreadSenders,
) {
    let _ = senders.send_to_plugin(PluginInstruction::PluginFailed {
        load_id: None,
        plugin_id,
        generation,
        instance_id,
        loading_client: None,
        error: message.clone(),
        failed_to_load: false,
    });
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
