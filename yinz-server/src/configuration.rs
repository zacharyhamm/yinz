//! Session configuration, live reconfiguration and persistence.
use super::*;
use yinz_config::config::{watch_config_file_changes, ConfigWriteError};
use yinz_types::keys::KeyWithModifier;

#[path = "configuration_editor.rs"]
mod configuration_editor;

fn validate_background_plugins(config: &Config) -> Result<()> {
    for declaration in &config.background_plugins {
        let mut declaration = declaration.clone();
        declaration.populate_run_plugin_if_needed(&config.plugins);
        let run = declaration
            .get_run_plugin()
            .with_context(|| format!("unknown load_plugins alias: {declaration:?}"))?;
        yinz_config::plugins::PluginConfig::from_run_plugin(&run)
            .with_context(|| format!("Failed to resolve plugin: {}", run.location))?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct BackgroundPluginConfiguration {
    declarations: std::collections::BTreeSet<RunPluginOrAlias>,
    cleared: bool,
    aliases: PluginAliases,
}

impl BackgroundPluginConfiguration {
    fn from_config(config: &Config) -> Self {
        let aliases = config
            .background_plugins
            .iter()
            .filter_map(|declaration| match declaration {
                RunPluginOrAlias::Alias(alias) => config
                    .plugins
                    .aliases
                    .get(&alias.name)
                    .map(|plugin| (alias.name.clone(), plugin.clone())),
                RunPluginOrAlias::RunPlugin(_) => None,
            })
            .collect();
        Self {
            declarations: config.background_plugins.clone(),
            cleared: config.background_plugins_cleared,
            aliases: PluginAliases::from_data(aliases),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SessionConfiguration {
    pending_editor_reload: Option<Config>,
    editor_overrides: Vec<yinz_types::configuration::Patch>,
    editor_effective: HashMap<ClientId, Config>,
    runtime_config: HashMap<ClientId, Config>, // if present, overrides the saved_config
    detached_runtime_config: Option<Config>,
    effective_keybinds: HashMap<ClientId, Keybinds>,
    plugin_default_keybinds: BTreeMap<PluginId, Keybinds>,
    background_plugins: Option<BackgroundPluginConfiguration>,
    pub(super) saved_config: Config, // the config as it is on disk (not guaranteed),
                                     // when changed, this resets the runtime config to
                                     // be identical to it and override any previous
                                     // changes
}

impl SessionConfiguration {
    pub fn change_saved_config(&mut self, new_saved_config: Config) -> Vec<(ClientId, Config)> {
        if self.pending_editor_reload.as_ref() == Some(&new_saved_config) {
            return vec![];
        }
        self.pending_editor_reload = None;
        self.background_plugins = Some(BackgroundPluginConfiguration::from_config(
            &new_saved_config,
        ));
        let previous_saved_config =
            std::mem::replace(&mut self.saved_config, new_saved_config.clone());
        let had_detached_runtime = self.detached_runtime_config.take().is_some();

        let mut config_changes = vec![];
        let client_ids: Vec<_> = self.runtime_config.keys().copied().collect();
        for client_id in client_ids {
            if self.runtime_config.get(&client_id) != Some(&new_saved_config) {
                self.runtime_config
                    .insert(client_id, new_saved_config.clone());
                config_changes.push((
                    client_id,
                    self.apply_editor_overrides(new_saved_config.clone()),
                ))
            }
        }
        // The last departing client's baseline may differ from the threads' current state.
        if self.runtime_config.is_empty()
            && (had_detached_runtime || previous_saved_config != new_saved_config)
        {
            config_changes.push((
                ClientId::NONE,
                self.apply_editor_overrides(new_saved_config),
            ));
        }
        self.recompute_effective_keybinds();
        config_changes
    }
    pub fn set_saved_configuration(&mut self, config: Config) {
        self.background_plugins = Some(BackgroundPluginConfiguration::from_config(&config));
        self.saved_config = config;
        self.detached_runtime_config = None;
        self.recompute_effective_keybinds();
    }
    pub fn set_client_runtime_configuration(&mut self, client_id: ClientId, client_config: Config) {
        self.detached_runtime_config = None;
        self.runtime_config.insert(client_id, client_config);
        self.recompute_effective_keybinds();
    }
    /// Retain the last client's runtime baseline as session state, without its client identity.
    pub fn remove_client(&mut self, client_id: ClientId) {
        let departed = self.runtime_config.remove(&client_id);
        if self.runtime_config.is_empty() && departed.is_some() {
            self.detached_runtime_config = departed;
        }
        self.editor_effective.remove(&client_id);
        self.effective_keybinds.remove(&client_id);
    }
    pub fn get_client_keybinds(&self, client_id: &ClientId) -> &Keybinds {
        self.runtime_config
            .get(client_id)
            .and_then(|_| self.effective_keybinds.get(client_id))
            .or_else(|| {
                self.detached_runtime_config
                    .as_ref()
                    .filter(|_| *client_id == ClientId::NONE)
                    .map(|config| &config.keybinds)
            })
            .unwrap_or(&self.saved_config.keybinds)
    }
    pub fn get_client_default_input_mode(&self, client_id: &ClientId) -> InputMode {
        self.editor_effective
            .get(client_id)
            .map(|config| config.options.default_mode)
            .unwrap_or_else(|| {
                self.get_client_configuration(client_id)
                    .options
                    .default_mode
            })
            .unwrap_or_default()
    }
    pub fn get_client_locked_escape_timeout(&self, client_id: &ClientId) -> Duration {
        let millis = self
            .editor_effective
            .get(client_id)
            .map(|config| config.options.locked_escape_timeout_ms)
            .unwrap_or_else(|| {
                self.get_client_configuration(client_id)
                    .options
                    .locked_escape_timeout_ms
            })
            .unwrap_or(yinz_types::units::ElapsedMillis::new(
                locked_escape::DEFAULT_LOCKED_ESCAPE_TIMEOUT_MS,
            ));
        Duration::from_millis((millis).get())
    }
    pub fn get_client_configuration(&self, client_id: &ClientId) -> Config {
        if let Some(config) = self.editor_effective.get(client_id) {
            return config.clone();
        }
        self.apply_editor_overrides(self.base_client_configuration(client_id))
    }
    fn base_client_configuration(&self, client_id: &ClientId) -> Config {
        let mut config = self
            .runtime_config
            .get(client_id)
            .or_else(|| {
                self.detached_runtime_config
                    .as_ref()
                    .filter(|_| *client_id == ClientId::NONE)
            })
            .unwrap_or(&self.saved_config)
            .clone();
        if let Some(background) = &self.background_plugins {
            config.background_plugins = background.declarations.clone();
            config.background_plugins_cleared = background.cleared;
            config.plugins.merge(background.aliases.clone());
        }
        config
    }
    pub fn reconfigure_runtime_config(
        &mut self,
        client_id: &ClientId,
        stringified_config: String,
        config_file_path: Option<&Path>,
        config_dir: Option<&Path>,
    ) -> Result<(Option<Config>, bool)> {
        let current = self.get_client_configuration(client_id);
        let base = self.base_client_configuration(client_id);
        let new_config = Config::from_kdl(&stringified_config, Some(base))
            .and_then(|config| config.resolve_plugin_repositories(config_file_path, config_dir))
            .map_err(|error| anyhow!("{error}"))?;
        validate_background_plugins(&new_config)?;
        self.background_plugins = Some(BackgroundPluginConfiguration::from_config(&new_config));
        self.runtime_config.insert(*client_id, new_config.clone());
        self.recompute_effective_keybinds();
        let effective = self.get_client_configuration(client_id);
        let changed = current != effective;
        self.forget_transient(client_id);
        Ok((Some(effective), changed))
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
            let base = self.base_client_configuration(client_id);
            self.runtime_config.insert(*client_id, base);
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
            },
            None => {
                log::error!(
                    "Could not find runtime or saved configuration for client, cannot rebind keys"
                );
            },
        }
        if config_changed {
            self.recompute_effective_keybinds();
            full_reconfigured_config = Some(self.get_client_configuration(client_id));
        }
        self.forget_transient(client_id);

        (full_reconfigured_config, config_changed)
    }
    /// Keep detached changes as session state without retaining a synthetic client entry.
    fn forget_transient(&mut self, client_id: &ClientId) {
        if *client_id == ClientId::NONE {
            self.detached_runtime_config = self.runtime_config.remove(client_id);
            self.remove_client(ClientId::NONE);
        }
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
        self.editor_effective.clear();
        self.editor_effective = self
            .runtime_config
            .keys()
            .map(|id| (*id, self.get_client_configuration(id)))
            .collect();
        self.refresh_effective_keybinds();
    }

    fn refresh_effective_keybinds(&mut self) {
        self.effective_keybinds = self
            .editor_effective
            .iter()
            .map(|(id, config)| (*id, self.with_plugin_defaults(config.clone()).keybinds))
            .collect();
    }
}

impl SessionMetaData {
    /// Tell the plugins that a config write failed: the file, when one was reached, and why,
    /// when it is known.
    fn report_config_write_failure(&self, file_path: Option<PathBuf>, reason: Option<String>) {
        self.senders
            .send_to_plugin(PluginInstruction::FailedToWriteConfigToDisk { file_path, reason })
            .context("reporting a failed config write to the plugins")
            .non_fatal();
    }

    /// Reconfigure every thread for the clients whose configuration changed, each with its
    /// plugins' default keybinds filled in, and cancel their pending locked escapes.
    pub fn propagate_configuration_changes(
        &mut self,
        config_changes: Vec<(ClientId, Config)>,
        config_was_written_to_disk: bool,
    ) {
        let mut effective_changes = Vec::with_capacity(config_changes.len());
        for (client_id, new_config) in config_changes {
            self.locked_escapes.remove(&client_id);
            let new_config = self.session_configuration.with_plugin_defaults(new_config);
            if client_id != ClientId::NONE {
                self.pending_mouse_updates
                    .push((client_id, new_config.options.mouse_mode.unwrap_or(true)));
            }
            effective_changes.push((client_id, new_config));
        }
        if let Some((_, last_config)) = effective_changes.last() {
            self.default_shell = thread_bus::default_shell_action(&last_config.options);
        }
        self.senders.reconfigure_threads(
            effective_changes,
            self.config_dir.as_deref(),
            config_was_written_to_disk,
        );
    }
}

/// Where a configuration change made for a client lands. See [`configuration_target`].
struct ConfigurationTarget {
    /// That client while it is attached, otherwise [`ClientId::NONE`], which the threads apply
    /// session-wide.
    client_id: ClientId,
    /// The change is a departed client's while another client is attached, so only saving it
    /// may change the session: it is built on the saved config, and applied session-wide it
    /// would replace the settings the attached clients are running with.
    save_only: bool,
}

/// Where a configuration change made for `client_id` lands, or `None` when it is dropped: a
/// departed client's unsaved change while another client is attached. With nobody attached
/// there are no settings to replace, and the change applies session-wide.
fn configuration_target(
    client_id: ClientId,
    write_config_to_disk: bool,
    change: &str,
    session_state: &Arc<RwLock<SessionState>>,
) -> Result<Option<ConfigurationTarget>> {
    let session_state = session_state
        .read()
        .to_anyhow()
        .context("failed to read which clients are attached")?;
    if session_state.is_interactive(&client_id) {
        return Ok(Some(ConfigurationTarget {
            client_id,
            save_only: false,
        }));
    }
    let save_only = session_state.has_interactive_clients();
    if save_only && !write_config_to_disk {
        log::warn!(
            "dropping {change} for client {client_id}: it is not attached, the change is not \
             saved, and applying it would replace the attached clients' settings"
        );
        return Ok(None);
    }
    Ok(Some(ConfigurationTarget {
        client_id: ClientId::NONE,
        save_only,
    }))
}

/// `Reconfigure`, from a plugin or `yinz action`. A departed client's change applies
/// session-wide, as [`configuration_target`] decides.
pub(super) fn reconfigure_client(
    client_id: ClientId,
    config: String,
    write_config_to_disk: bool,
    session_state: &Arc<RwLock<SessionState>>,
    session_data: &Arc<RwLock<Option<SessionMetaData>>>,
) -> Result<()> {
    let Some(target) = configuration_target(
        client_id,
        write_config_to_disk,
        "a reconfiguration",
        session_state,
    )?
    else {
        return Ok(());
    };
    let Some(result) = with_session_mut(session_data, |session| {
        // A save-only change is worked out on a copy: the session's configuration changes only
        // once the change is saved, and then through the saved config.
        let mut staged = target
            .save_only
            .then(|| session.session_configuration.clone());
        staged
            .as_mut()
            .unwrap_or(&mut session.session_configuration)
            .reconfigure_runtime_config(
                &target.client_id,
                config.clone(),
                session.config_file_path.as_deref(),
                session.config_dir.as_deref(),
            )
    })?
    else {
        return Ok(());
    };
    let (new_config, changed) = result.context("failed to reconfigure")?;
    let persistence = match write_config_to_disk {
        true => ConfigPersistence::Fragment(config),
        false => ConfigPersistence::None,
    };
    update_new_saved_config(
        new_config,
        persistence,
        changed && !target.save_only,
        session_data,
        target.client_id,
    )
}

pub(super) fn reconfigure_all_clients(
    config: String,
    write_config_to_disk: bool,
    mut completion: Option<NotificationEnd>,
    session_state: &Arc<RwLock<SessionState>>,
    session_data: &Arc<RwLock<Option<SessionMetaData>>>,
) {
    let result = (|| -> Result<()> {
        let client_ids = session_state
            .read()
            .to_anyhow()
            .context("read clients for reconfiguration")?
            .interactive_client_ids();
        // Validate the whole change before mutating live configuration. Background
        // declarations are session-wide; notify the plugin thread once for the batch.
        let client_ids = if client_ids.is_empty() {
            vec![ClientId::NONE]
        } else {
            client_ids
        };
        if write_config_to_disk {
            let client_id = client_ids[0];
            let (new_config, changed) = with_session_mut(session_data, |session| {
                session.session_configuration.reconfigure_runtime_config(
                    &client_id,
                    config.clone(),
                    session.config_file_path.as_deref(),
                    session.config_dir.as_deref(),
                )
            })?
            .context("session is unavailable")??;
            update_new_saved_config(
                new_config,
                ConfigPersistence::Fragment(config.clone()),
                changed,
                session_data,
                client_id,
            )?;
        } else {
            with_session_mut(session_data, |session| -> Result<()> {
                let mut staged = session.session_configuration.clone();
                let mut changes = vec![];
                for client_id in client_ids {
                    let (new_config, changed) = staged.reconfigure_runtime_config(
                        &client_id,
                        config.clone(),
                        session.config_file_path.as_deref(),
                        session.config_dir.as_deref(),
                    )?;
                    if let (Some(new_config), true) = (new_config, changed) {
                        changes.push((client_id, new_config));
                    }
                }
                session.session_configuration = staged;
                session.propagate_configuration_changes(changes, false);
                Ok(())
            })?
            .context("session is unavailable")??;
        }
        if config.contains("load_plugins") {
            let senders = with_session(session_data, |session| session.senders.clone())?
                .context("session is unavailable")?;
            let (reply, receiver) = std::sync::mpsc::channel();
            senders
                .send_to_plugin(PluginInstruction::PluginRequest {
                    request: yinz_types::cli::PluginRequest {
                        operation: yinz_types::cli::PluginOperation::List,
                        target: None,
                        configuration: None,
                        fallback: None,
                        candidates: vec![],
                        caller: None,
                    },
                    client_id: ClientId::NONE,
                    reply,
                })
                .context("query scheduled background plugins")?;
            complete_plugin_reconfiguration(receiver, completion.take(), Duration::from_secs(30));
        }
        Ok(())
    })();
    if let Err(error) = result {
        if let Some(completion) = completion.as_mut() {
            completion.set_exit_status(1);
            completion.set_error_message(format!("{error:#}"));
        }
        log::error!("reconfigure: {error:#}");
    }
}

fn complete_plugin_reconfiguration(
    receiver: std::sync::mpsc::Receiver<
        std::result::Result<yinz_types::cli::PluginResponse, String>,
    >,
    mut completion: Option<NotificationEnd>,
    timeout: Duration,
) {
    // Keep draining the server queue while the plugin thread works toward its reply.
    global_async_runtime::get_tokio_runtime().spawn_blocking(move || {
        let result = receiver
            .recv_timeout(timeout)
            .context("background plugin scheduling timed out")
            .and_then(|response| response.map_err(|error| anyhow!("{error}")));
        match result {
            Ok(response) => {
                if let Some(completion) = completion.as_mut() {
                    completion.set_stdout_message(format!(
                        "Scheduled plugin state: {}",
                        response
                            .plugins
                            .iter()
                            .map(|plugin| format!("plugin_{} {:?}", plugin.id, plugin.state))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
            },
            Err(error) => {
                if let Some(completion) = completion.as_mut() {
                    completion.set_exit_status(1);
                    completion.set_error_message(format!("{error:#}"));
                }
                log::error!("reconfigure: {error:#}");
            },
        }
    });
}

/// `RebindKeys` for one client. A departed client's rebinding applies session-wide, as
/// [`configuration_target`] decides.
pub(super) fn rebind_client_keys(
    client_id: ClientId,
    keys_to_rebind: Vec<(InputMode, KeyWithModifier, Vec<Action>)>,
    keys_to_unbind: Vec<(InputMode, KeyWithModifier)>,
    write_config_to_disk: bool,
    session_state: &Arc<RwLock<SessionState>>,
    session_data: &Arc<RwLock<Option<SessionMetaData>>>,
) -> Result<()> {
    let Some(target) = configuration_target(
        client_id,
        write_config_to_disk,
        "a key rebinding",
        session_state,
    )?
    else {
        return Ok(());
    };
    let Some((new_config, changed)) = with_session_mut(session_data, |session| {
        // As in `reconfigure_client`: a save-only change does not touch the session until saved.
        let mut staged = target
            .save_only
            .then(|| session.session_configuration.clone());
        staged
            .as_mut()
            .unwrap_or(&mut session.session_configuration)
            .rebind_keys(&target.client_id, keys_to_rebind, keys_to_unbind)
    })?
    else {
        return Ok(());
    };
    update_new_saved_config(
        new_config,
        match write_config_to_disk {
            true => ConfigPersistence::WholeConfig,
            false => ConfigPersistence::None,
        },
        changed && !target.save_only,
        session_data,
        target.client_id,
    )
}

pub(super) fn report_changes_in_config_file(
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

/// Stage the theme change without touching live state; only a successful write may be applied.
#[cfg(test)]
fn save_palette_theme(
    configuration: &SessionConfiguration,
    client: ClientId,
    name: &str,
    config_file_path: Option<&Path>,
    config_dir: Option<&Path>,
    cancelled: &std::sync::atomic::AtomicBool,
) -> Result<Config> {
    let current = configuration.get_client_configuration(&client);
    let fragment = theme_fragment(&current, name)?;
    let mut staged = configuration.clone();
    let (config, _) = staged
        .reconfigure_runtime_config(&client, fragment.clone(), config_file_path, config_dir)
        .context("validating theme configuration")?;
    let config = config.context("theme configuration is unavailable")?;
    let dir = yinz_runtime::home::get_config_overlay_dir(
        config_dir
            .map(Path::to_path_buf)
            .or_else(yinz_runtime::home::find_default_config_dir),
    )
    .context("no config directory to save the theme in")?;
    if cancelled.load(std::sync::atomic::Ordering::Acquire) {
        return Err(anyhow!("Theme selection was cancelled"));
    }
    Config::try_write_managed_fragment_guarded(&dir, &fragment, || {
        if cancelled.load(std::sync::atomic::Ordering::Acquire) {
            Err("Theme selection was cancelled".into())
        } else {
            Ok(())
        }
    })
    .map_err(|error| anyhow!("{error}"))
    .context("saving theme")?;
    Ok(config)
}

fn theme_fragment(config: &Config, name: &str) -> Result<String> {
    if config.themes.get_theme(name).is_none() {
        return Err(anyhow!("Theme '{name}' is no longer available"));
    }
    let options = yinz_types::input::options::Options {
        theme: Some(name.into()),
        theme_dark: config.options.theme_dark.as_ref().map(|_| name.into()),
        theme_light: config.options.theme_light.as_ref().map(|_| name.into()),
        ..Default::default()
    };
    Ok(yinz_config::kdl::options::to_kdl(&options, false)
        .into_iter()
        .map(|node| node.to_string())
        .collect::<Vec<_>>()
        .join("\n"))
}

/// How a configuration change is kept, beyond the running session.
enum ConfigPersistence {
    /// Applied to the session only.
    None,
    /// The whole effective config rewrites the user's config file. Only `RebindKeys` still
    /// does this - it has no fragment to write.
    WholeConfig,
    /// The requested KDL fragment goes into `config.d/yinz-managed.kdl`, whatever the
    /// user's own config is written in. See `Config::write_managed_fragment`.
    Fragment(String),
}

fn update_new_saved_config(
    new_config: Option<Config>,
    persistence: ConfigPersistence,
    runtime_config_changed: bool,
    session_data: &Arc<RwLock<Option<SessionMetaData>>>,
    client_id: ClientId,
) -> Result<()> {
    if let Some(new_config) = new_config {
        let propagate_saved = |session: &mut SessionMetaData, config: Config| {
            let mut changes = session
                .session_configuration
                .change_saved_config(config.clone());
            // The caller already changed this client's runtime config. Saving therefore omits
            // it from the diff, although its screen and plugins have not been notified yet.
            if runtime_config_changed && !changes.iter().any(|(id, _)| *id == client_id) {
                changes.push((client_id, config));
            }
            session.propagate_configuration_changes(changes, true);
        };
        if let ConfigPersistence::Fragment(fragment) = persistence {
            let overlay_dir = {
                let config_dir =
                    with_session(session_data, |session| session.config_dir.clone())?.flatten();
                yinz_runtime::home::get_config_overlay_dir(
                    config_dir.or_else(yinz_runtime::home::find_default_config_dir),
                )
            };
            let written = match overlay_dir {
                Some(dir) => Config::write_managed_fragment(&dir, &fragment).map_err(|e| {
                    log::error!("Failed to persist the config fragment: {}", e);
                    (
                        Some(dir.join(yinz_config::config::MANAGED_CONFIG_FILE_NAME)),
                        e.to_string(),
                    )
                }),
                None => {
                    let reason = "no config directory to persist the config fragment in";
                    log::error!("{reason}");
                    Err((None, reason.to_owned()))
                },
            };
            match written {
                Ok(_) => {
                    // The session's saved config is the running one now, and every client
                    // hears that it was written.
                    with_session_mut(session_data, |session| propagate_saved(session, new_config))?;
                },
                Err((file_path, reason)) => {
                    with_session(session_data, |session| {
                        session.report_config_write_failure(file_path, Some(reason.clone()))
                    })?;
                    if runtime_config_changed {
                        with_session_mut(session_data, |session| {
                            session.propagate_configuration_changes(
                                vec![(client_id, new_config)],
                                false,
                            )
                        })?;
                    }
                    return Err(anyhow!("{reason}"));
                },
            }
            return Ok(());
        }
        if matches!(persistence, ConfigPersistence::WholeConfig) {
            let add_comments = true;
            let config_file_path =
                with_session(session_data, |session| session.config_file_path.clone())?.flatten();

            let Some(config_file_path) = config_file_path.as_ref() else {
                let reason = "the session has no config file path to write to";
                log::error!("{reason}");
                with_session(session_data, |session| {
                    session.report_config_write_failure(None, Some(reason.to_owned()))
                })?;
                return Err(anyhow!("{reason}"));
            };
            // Serialized first, on its own: a binding whose action has no config spelling
            // refuses the save here, with the mode, key and action in the reason, before
            // anything on disk is touched.
            let written = new_config
                .to_string(add_comments)
                .map_err(|e| ConfigWriteError {
                    path: None,
                    reason: e.to_string(),
                })
                .and_then(|text| new_config.write_config_to_disk(text, config_file_path));
            match written {
                Ok(written_config) => {
                    with_session_mut(session_data, |session| {
                        propagate_saved(session, written_config)
                    })?;
                },
                Err(ConfigWriteError { path, reason }) => {
                    Err::<(), _>(anyhow!("{reason}"))
                        .with_context(|| match &path {
                            Some(path) => {
                                format!("refusing to save the configuration to {}", path.display())
                            },
                            None => "refusing to save the configuration".to_owned(),
                        })
                        .non_fatal();
                    with_session(session_data, |session| {
                        session.report_config_write_failure(path, Some(reason.clone()))
                    })?;
                    return Err(anyhow!("{reason}"));
                },
            }
        } else if runtime_config_changed {
            let config_was_written_to_disk = false;
            with_session_mut(session_data, |session| {
                session.propagate_configuration_changes(
                    vec![(client_id, new_config)],
                    config_was_written_to_disk,
                )
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "unit/session_reconfigure_tests.rs"]
mod session_reconfigure_tests;
#[cfg(test)]
#[path = "unit/session_state_tests.rs"]
mod session_state_tests;
#[cfg(test)]
#[path = "unit/session_teardown_tests.rs"]
mod session_teardown_tests;
