//! Session configuration, live reconfiguration and persistence.
use super::*;
use yinz_config::config::{watch_config_file_changes, ManagedConfigRevision};
use yinz_config::screen_config::ApplicationThemeConfig;
use yinz_types::input::native_terminal::TerminalConfig;
use yinz_types::keys::KeyWithModifier;

#[path = "configuration_editor.rs"]
mod configuration_editor;

fn validate_unsaved_native_settings(
    current: &Config,
    requested: &Config,
    saved_terminal_changes: &[&str],
) -> Result<()> {
    let mut changed = saved_terminal_changes.to_vec();
    if current.terminal.font_ligatures.unwrap_or(true)
        != requested.terminal.font_ligatures.unwrap_or(true)
    {
        changed.push("ligatures/font_ligatures");
    }
    if current.terminal.cell_width.unwrap_or(1.0) != requested.terminal.cell_width.unwrap_or(1.0) {
        changed.push("cell_width");
    }
    if current.terminal.line_height.unwrap_or(1.0) != requested.terminal.line_height.unwrap_or(1.0)
    {
        changed.push("line_height");
    }
    if current.terminal.baseline_offset.unwrap_or(0)
        != requested.terminal.baseline_offset.unwrap_or(0)
    {
        changed.push("baseline_offset");
    }
    if effective_font_features(&current.terminal) != effective_font_features(&requested.terminal) {
        changed.push("font_features");
    }
    changed.extend(changed_padding_settings(
        &current.terminal,
        &requested.terminal,
    ));
    if !changed.is_empty() {
        return Err(anyhow!(
            "changing terminal {} requires a saved configuration: \
             repeat action reconfigure with --write, or edit the session's configuration/drop-in \
             for automatic file reload; the entire fragment was refused",
            changed.join(", ")
        ));
    }
    Ok(())
}

/// Names the padding keys behind a change in effective padding. Native windows
/// read padding only from files, so an unsaved change would be a silent no-op.
/// A changed side is named by its own key when that key differs, otherwise by
/// the `padding` shorthand that produced it.
fn changed_padding_settings(
    current: &TerminalConfig,
    requested: &TerminalConfig,
) -> Vec<&'static str> {
    let (before, after) = (current.padding_sides(), requested.padding_sides());
    let sides = [
        (
            "padding_top",
            before.top != after.top,
            current.padding_top != requested.padding_top,
        ),
        (
            "padding_right",
            before.right != after.right,
            current.padding_right != requested.padding_right,
        ),
        (
            "padding_bottom",
            before.bottom != after.bottom,
            current.padding_bottom != requested.padding_bottom,
        ),
        (
            "padding_left",
            before.left != after.left,
            current.padding_left != requested.padding_left,
        ),
    ];
    let mut changed = Vec::new();
    if sides
        .iter()
        .any(|&(_, effective, explicit)| effective && !explicit)
    {
        changed.push("padding");
    }
    changed.extend(
        sides
            .iter()
            .filter(|&&(_, effective, explicit)| effective && explicit)
            .map(|&(name, ..)| name),
    );
    if current.padding_balance_enabled() != requested.padding_balance_enabled() {
        changed.push("padding_balance");
    }
    changed
}

fn changed_saved_terminal_settings(
    requested: &TerminalConfig,
    current: &TerminalConfig,
) -> impl Iterator<Item = &'static str> {
    let cursor_trail = requested
        .cursor_trail
        .filter(|value| *value != current.cursor_trail.unwrap_or(true))
        .map(|_| "cursor_trail");
    [
        (
            "underline_offset",
            requested.underline_offset,
            current.underline_offset,
        ),
        (
            "underline_thickness",
            requested.underline_thickness,
            current.underline_thickness,
        ),
    ]
    .into_iter()
    .filter_map(|(name, requested, current)| {
        requested
            .filter(|value| *value != current.unwrap_or(0))
            .map(|_| name)
    })
    .chain(cursor_trail)
}

struct SavedTerminalReconfigure {
    requested: TerminalConfig,
    saved_changes: Result<Vec<&'static str>>,
}

impl SavedTerminalReconfigure {
    fn read(config: &str, session_data: &Arc<RwLock<Option<SessionMetaData>>>) -> Result<Self> {
        let requested = Config::terminal_settings_from_kdl(config)
            .map_err(|error| anyhow!(yinz_config::editor::validation_error(error)))?
            .unwrap_or_default();
        let names = [
            requested.underline_offset.map(|_| "underline_offset"),
            requested.underline_thickness.map(|_| "underline_thickness"),
            requested.cursor_trail.map(|_| "cursor_trail"),
        ];
        let saved_changes = if names.iter().any(Option::is_some) {
            let (file, dir) = with_session(session_data, |session| {
                (session.config_file_path.clone(), session.config_dir.clone())
            })?
            .context("session is unavailable")?;
            Config::load_saved(file.as_deref(), dir.as_deref())
                .map(|saved| changed_saved_terminal_settings(&requested, &saved.terminal).collect())
                .map_err(|error| anyhow!(error.diagnostic_message()))
                .with_context(|| format!(
                    "cannot compare terminal {} with the saved configuration; the entire fragment was refused. \
                     Repair or restore the reported file for automatic reload. \
                     Use --write to attempt to save the fragment in the session's managed drop-in",
                    names.into_iter().flatten().collect::<Vec<_>>().join(", ")
                ))
        } else {
            Ok(vec![])
        };
        Ok(Self {
            requested,
            saved_changes,
        })
    }

    fn defer_save(&self, current: &TerminalConfig) -> bool {
        !self.saved_changes.as_ref().is_ok_and(Vec::is_empty)
            || changed_saved_terminal_settings(&self.requested, current)
                .next()
                .is_some()
    }
}

fn inspect_startup_axes(fragment: &str, write: bool) -> Result<bool> {
    let (axes, axes_only) = Config::startup_axes_in_fragment(fragment)
        .map_err(|error| anyhow!(yinz_config::editor::validation_error(error)))?;
    let explicit: Vec<_> = ["initial_columns", "initial_rows"]
        .into_iter()
        .zip(axes)
        .filter_map(|(name, value)| value.map(|_| name))
        .collect();
    anyhow::ensure!(
        write || explicit.is_empty(),
        "setting terminal {} requires a saved configuration: repeat action reconfigure with --write, or edit the host configuration/drop-in; the entire fragment was refused",
        explicit.join(", ")
    );
    Ok(axes_only)
}

/// The feature list as the shaper sees it: omission and an empty list both mean
/// none, and `calt` equals `+calt`. Order stays significant.
fn effective_font_features(
    terminal: &yinz_types::input::native_terminal::TerminalConfig,
) -> Vec<std::result::Result<([u8; 4], u16), &str>> {
    terminal
        .font_features
        .iter()
        .flatten()
        .map(|text| {
            yinz_types::input::native_terminal::parse_font_feature(text).ok_or(text.as_str())
        })
        .collect()
}

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
    startup_axes_save: Option<ManagedConfigRevision>,
    editor_overrides: Vec<yinz_types::configuration::Patch>,
    editor_effective: HashMap<ClientId, Config>,
    runtime_config: HashMap<ClientId, Config>, // if present, overrides the saved_config
    detached_runtime_config: Option<Config>,
    effective_keybinds: HashMap<ClientId, Keybinds>,
    plugin_default_keybinds: BTreeMap<PluginId, Keybinds>,
    background_plugins: Option<BackgroundPluginConfiguration>,
    /// The session's own theme definition, which shared application color answers and the
    /// pane palette resolve from (`docs/TERMINAL_COLORS.md`). It changes only on
    /// session-wide paths: disk reloads, saved or session-wide fragments and editor edits.
    /// One client's attach options or unsaved per-client reconfigure never reach it.
    /// `None` follows `saved_config` until something changes either of them.
    application_theme_source: Option<Config>,
    /// The application theme the screen holds. `None` until the session is initialized.
    published_application_theme: Option<ApplicationThemeConfig>,
    pub(super) saved_config: Config, // the config as it is on disk (not guaranteed),
                                     // when changed, this resets the runtime config to
                                     // be identical to it and override any previous
                                     // changes
}

impl SessionConfiguration {
    /// Replace the saved config. This never changes the session theme source: on the
    /// `--write` path `new_saved_config` is one client's effective config, attach options
    /// included. A disk reload goes through [`Self::reload_saved_config`].
    pub fn change_saved_config(&mut self, new_saved_config: Config) -> Vec<(ClientId, Config)> {
        if self.pending_editor_reload.as_ref() == Some(&new_saved_config) {
            return vec![];
        }
        self.materialize_application_theme_source();
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
    /// The config file changed on disk. Unless this is the echo of a write the session
    /// already applied (an editor or keybinding save), the disk config becomes the session
    /// theme source.
    pub fn reload_saved_config(&mut self, config: Config) -> Vec<(ClientId, Config)> {
        if let Some(expected) = &self.pending_editor_reload {
            // Editor and rebind echoes can coalesce with an independent axes save.
            let mut observed = config.clone();
            observed.terminal.initial_columns = expected.terminal.initial_columns;
            observed.terminal.initial_rows = expected.terminal.initial_rows;
            if &observed == expected {
                return vec![];
            }
        }
        self.application_theme_source = Some(config.clone());
        self.change_saved_config(config)
    }
    pub fn set_saved_configuration(&mut self, config: Config) {
        self.background_plugins = Some(BackgroundPluginConfiguration::from_config(&config));
        self.saved_config = config;
        self.detached_runtime_config = None;
        self.recompute_effective_keybinds();
        self.initialize_application_theme(self.saved_config.clone());
    }
    /// Start the session theme source from `source` (the creating client's merged config),
    /// which is what the screen resolved its initial baseline from, so nothing is sent.
    pub fn initialize_application_theme(&mut self, source: Config) {
        self.application_theme_source = Some(source);
        self.published_application_theme = Some(self.application_theme_configuration());
    }
    fn application_theme_source(&self) -> &Config {
        self.application_theme_source
            .as_ref()
            .unwrap_or(&self.saved_config)
    }
    /// Pin the source before `saved_config` changes on a path that must not move it.
    fn materialize_application_theme_source(&mut self) {
        if self.application_theme_source.is_none() {
            self.application_theme_source = Some(self.saved_config.clone());
        }
    }
    /// The session theme the screen resolves shared application answers from.
    pub fn application_theme_configuration(&self) -> ApplicationThemeConfig {
        ApplicationThemeConfig::from_config(
            &self.apply_editor_overrides(self.application_theme_source().clone()),
        )
    }
    /// The session theme source with a session-wide KDL `fragment` applied. A parse
    /// failure reports the same actionable reason as the runtime reconfigure.
    fn application_theme_source_with(&self, fragment: &str) -> Result<Config> {
        Config::from_kdl(fragment, Some(self.application_theme_source().clone()))
            .map_err(|error| anyhow!(yinz_config::editor::validation_error(error)))
    }
    fn set_application_theme_source(&mut self, source: Config) {
        self.application_theme_source = Some(source);
    }
    /// Record the application theme the screen must hold now, returning it when it
    /// changed. An uninitialized session records it silently: its screen resolved the
    /// same value from the startup config.
    fn take_application_theme_change(&mut self) -> Option<ApplicationThemeConfig> {
        let current = self.application_theme_configuration();
        let changed = self
            .published_application_theme
            .as_ref()
            .is_some_and(|published| *published != current);
        if self.published_application_theme.as_ref() != Some(&current) {
            self.published_application_theme = Some(current.clone());
        }
        changed.then_some(current)
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
            .map_err(|error| anyhow!(yinz_config::editor::validation_error(error)))?;
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
    /// Mark the keybindings a save wrote as saved. `saved` is the saved config with `changes`
    /// applied the way the loader applies them, so it equals what the file now loads. Every
    /// client's and the detached runtime config get the same bindings; their other runtime
    /// settings are kept, unsaved. Like the configuration editor's permanent save, this arms
    /// `pending_editor_reload`, so the watcher's reload of this same write changes nothing.
    /// Returns the clients whose effective configuration changed, or [`ClientId::NONE`] when no
    /// client is attached and the session-wide configuration changed.
    fn commit_saved_keybinds(
        &mut self,
        saved: Config,
        changes: &[yinz_config::editor::KeybindingChange],
    ) -> Vec<(ClientId, Config)> {
        let mut clients: Vec<_> = self.runtime_config.keys().copied().collect();
        if clients.is_empty() {
            clients.push(ClientId::NONE);
        }
        let before: Vec<_> = clients
            .iter()
            .map(|client| self.get_client_configuration(client))
            .collect();
        for config in self
            .runtime_config
            .values_mut()
            .chain(self.detached_runtime_config.iter_mut())
        {
            for change in changes {
                let bindings = config.keybinds.0.entry(change.mode).or_default();
                match saved
                    .keybinds
                    .get_actions_for_key_in_mode(&change.mode, &change.key)
                {
                    Some(actions) => {
                        bindings.insert(change.key.clone(), actions.clone());
                    },
                    None => {
                        bindings.remove(&change.key);
                    },
                }
            }
        }
        self.materialize_application_theme_source();
        self.pending_editor_reload = Some(saved.clone());
        self.saved_config = saved;
        self.recompute_effective_keybinds();
        clients
            .into_iter()
            .zip(before)
            .filter_map(|(client, before)| {
                let after = self.get_client_configuration(&client);
                (after != before).then_some((client, after))
            })
            .collect()
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
    /// Send the session's application theme to the screen when it changed. Called before
    /// any `Reconfigure` it accompanies and before a theme save closes its preview.
    pub(crate) fn publish_application_theme(&mut self) {
        if let Some(config) = self.session_configuration.take_application_theme_change() {
            self.senders
                .send_to_screen(ScreenInstruction::SetApplicationThemeConfig(config))
                .context("publishing the session's application theme")
                .non_fatal();
        }
    }

    pub(super) fn publish_disk_config(&mut self, change: yinz_config::config::DiskConfigChange) {
        let matches_axes_save = change.unchanged_managed_revision.is_some()
            && change.unchanged_managed_revision == self.session_configuration.startup_axes_save;
        if change.requires_live_reload && !matches_axes_save {
            let changes = self
                .session_configuration
                .reload_saved_config(change.config);
            self.propagate_configuration_changes(changes, true);
        }
    }

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
        self.publish_application_theme();
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
) -> Result<bool> {
    let Some(target) = configuration_target(
        client_id,
        write_config_to_disk,
        "a reconfiguration",
        session_state,
    )?
    else {
        return Ok(false);
    };
    if inspect_startup_axes(&config, write_config_to_disk)? {
        return save_startup_axes(&config, session_data).map(|()| true);
    }
    reconfigure_target(target, config, write_config_to_disk, session_data)?;
    // A session theme change can come without a per-client difference.
    with_session_mut(session_data, |session| session.publish_application_theme())?;
    Ok(false)
}

fn reconfigure_target(
    target: ConfigurationTarget,
    config: String,
    write_config_to_disk: bool,
    session_data: &Arc<RwLock<Option<SessionMetaData>>>,
) -> Result<Option<()>> {
    let saved_terminal = SavedTerminalReconfigure::read(&config, session_data)?;
    let Some(result) = with_session_mut(session_data, |session| {
        // Unsaved changes must be validated before any live state (including editor and
        // background-plugin state) changes. Save-only requests retain their existing policy.
        let mut staged = session.session_configuration.clone();
        let result = staged.reconfigure_runtime_config(
            &target.client_id,
            config.clone(),
            session.config_file_path.as_deref(),
            session.config_dir.as_deref(),
        )?;
        let current = session
            .session_configuration
            .get_client_configuration(&target.client_id);
        let defer = write_config_to_disk && saved_terminal.defer_save(&current.terminal);
        if !write_config_to_disk {
            validate_unsaved_native_settings(
                &current,
                &staged.get_client_configuration(&target.client_id),
                &saved_terminal.saved_changes?,
            )?;
            // With nobody attached the change is session-wide, so it is the session's
            // theme too. A saved change reaches the source once it is written.
            if target.client_id == ClientId::NONE && !target.save_only {
                let source = staged.application_theme_source_with(&config)?;
                staged.set_application_theme_source(source);
            }
        }
        let deferred = if target.save_only {
            None
        } else if defer {
            Some(staged)
        } else {
            session.session_configuration = staged;
            None
        };
        Ok::<_, anyhow::Error>((result, deferred))
    })?
    else {
        return Ok(None);
    };
    let ((new_config, changed), deferred) = result.context("failed to reconfigure")?;
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
        deferred,
    )?;
    Ok(Some(()))
}

pub(super) fn reconfigure_all_clients(
    config: String,
    write_config_to_disk: bool,
    mut completion: Option<NotificationEnd>,
    session_state: &Arc<RwLock<SessionState>>,
    session_data: &Arc<RwLock<Option<SessionMetaData>>>,
) -> bool {
    let result = (|| -> Result<bool> {
        if inspect_startup_axes(&config, write_config_to_disk)? {
            return save_startup_axes(&config, session_data).map(|()| true);
        }
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
            reconfigure_target(
                ConfigurationTarget {
                    client_id: client_ids[0],
                    save_only: false,
                },
                config.clone(),
                true,
                session_data,
            )?
            .context("session is unavailable")?;
        } else {
            let saved_terminal = SavedTerminalReconfigure::read(&config, session_data)?;
            with_session_mut(session_data, |session| -> Result<()> {
                let mut staged = session.session_configuration.clone();
                // `yinz action reconfigure` is session-wide, so it is the session's theme too.
                let source = staged.application_theme_source_with(&config)?;
                staged.set_application_theme_source(source);
                let mut changes = vec![];
                for client_id in client_ids {
                    let (new_config, changed) = staged.reconfigure_runtime_config(
                        &client_id,
                        config.clone(),
                        session.config_file_path.as_deref(),
                        session.config_dir.as_deref(),
                    )?;
                    validate_unsaved_native_settings(
                        &session
                            .session_configuration
                            .get_client_configuration(&client_id),
                        &staged.get_client_configuration(&client_id),
                        saved_terminal
                            .saved_changes
                            .as_deref()
                            .map_err(|error| anyhow!("{error:#}"))?,
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
        // A session theme change can come without a per-client difference.
        with_session_mut(session_data, |session| session.publish_application_theme())?;
        Ok(false)
    })();
    match result {
        Ok(notify_disk_write) => notify_disk_write,
        Err(error) => {
            if let Some(completion) = completion.as_mut() {
                completion.set_exit_status(1);
                completion.set_error_message(format!("{error:#}"));
            }
            log::error!("reconfigure: {error:#}");
            false
        },
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
    if write_config_to_disk {
        return save_rebound_keys(target, keys_to_rebind, keys_to_unbind, session_data);
    }
    // An unsaved change is never save-only: `configuration_target` drops those.
    let Some((new_config, changed)) = with_session_mut(session_data, |session| {
        session
            .session_configuration
            .rebind_keys(&target.client_id, keys_to_rebind, keys_to_unbind)
    })?
    else {
        return Ok(());
    };
    update_new_saved_config(
        new_config,
        ConfigPersistence::None,
        changed,
        session_data,
        target.client_id,
        None,
    )
}

/// A saved `RebindKeys`. The rebinding is worked out on a copy of the session configuration,
/// and what is written is every keybinding the client's runtime config then differs from the
/// saved config by - not only the requested keys - so bindings an earlier unsaved rebinding or
/// preset changed are saved as well, and the file's keybindings equal the client's afterwards.
/// They go to `config.d/yinz-managed.kdl` as per-binding edits; the user's `config.kdl` is not
/// rewritten. Only those keybindings are then marked saved: other unsaved runtime changes stay
/// applied and unsaved, because they are not on disk.
fn save_rebound_keys(
    target: ConfigurationTarget,
    keys_to_rebind: Vec<(InputMode, KeyWithModifier, Vec<Action>)>,
    keys_to_unbind: Vec<(InputMode, KeyWithModifier)>,
    session_data: &Arc<RwLock<Option<SessionMetaData>>>,
) -> Result<()> {
    use yinz_config::editor;
    struct Staged {
        configuration: SessionConfiguration,
        new_config: Option<Config>,
        changed: bool,
        changes: Vec<editor::KeybindingChange>,
        saved: Config,
        fragment: String,
    }
    let Some(staged) = with_session_mut(session_data, |session| -> Result<Staged, String> {
        let mut configuration = session.session_configuration.clone();
        let (new_config, changed) =
            configuration.rebind_keys(&target.client_id, keys_to_rebind, keys_to_unbind);
        let changes = editor::keybinding_changes(
            &configuration.saved_config.keybinds,
            &configuration
                .base_client_configuration(&target.client_id)
                .keybinds,
        );
        // Serialized first: a binding whose action has no config spelling refuses the save
        // here, with the mode, key and action in the reason, before anything changes.
        let patches = editor::keybinding_patches(&changes)?;
        let saved = editor::apply(&configuration.saved_config, &patches)?;
        let fragment = yinz_config::editor::patches_to_kdl(&patches)?.to_string();
        Ok(Staged {
            configuration,
            new_config,
            changed,
            changes,
            saved,
            fragment,
        })
    })?
    else {
        return Ok(());
    };
    let staged = match staged {
        Ok(staged) => staged,
        Err(reason) => {
            Err::<(), _>(anyhow!("{reason}"))
                .context("refusing to save the key rebinding")
                .non_fatal();
            with_session(session_data, |session| {
                session.report_config_write_failure(None, Some(reason.clone()))
            })?;
            return Err(anyhow!("{reason}"));
        },
    };
    let apply_unsaved = |session: &mut SessionMetaData, configuration: SessionConfiguration| {
        session.session_configuration = configuration;
        if let (Some(new_config), true) = (staged.new_config.clone(), staged.changed) {
            session.propagate_configuration_changes(vec![(target.client_id, new_config)], false);
        }
    };
    if staged.changes.is_empty() {
        // The saved keybindings already are the client's: there is nothing to write.
        if !target.save_only {
            with_session_mut(session_data, |session| {
                apply_unsaved(session, staged.configuration)
            })?;
        }
        return Ok(());
    }
    match write_managed_config_fragment(session_data, &staged.fragment)? {
        Ok(_) => {
            with_session_mut(session_data, |session| {
                if !target.save_only {
                    session.session_configuration = staged.configuration;
                }
                let mut changes = session
                    .session_configuration
                    .commit_saved_keybinds(staged.saved, &staged.changes);
                // The rebinding already changed this client's runtime config, so the commit
                // may not report it, but its screen and plugins have not heard of it yet.
                if !target.save_only && !changes.iter().any(|(id, _)| *id == target.client_id) {
                    changes.push((
                        target.client_id,
                        session
                            .session_configuration
                            .get_client_configuration(&target.client_id),
                    ));
                }
                session.propagate_configuration_changes(changes, true);
            })?;
            Ok(())
        },
        Err((file_path, reason)) => {
            with_session(session_data, |session| {
                session.report_config_write_failure(file_path, Some(reason.clone()))
            })?;
            // As with any failed fragment save, the change stays applied, unsaved.
            if !target.save_only {
                with_session_mut(session_data, |session| {
                    apply_unsaved(session, staged.configuration)
                })?;
            }
            Err(anyhow!("{reason}"))
        },
    }
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

/// How a configuration change is kept, beyond the running session. Nothing rewrites the
/// user's own config file: a saved `RebindKeys` writes per-binding edits to the same managed
/// file (see `save_rebound_keys`).
enum ConfigPersistence {
    /// Applied to the session only.
    None,
    /// The requested KDL fragment goes into `config.d/yinz-managed.kdl`, whatever the
    /// user's own config is written in. See `Config::write_managed_fragment`.
    Fragment(String),
}

/// Merge `fragment` into the session's `config.d/yinz-managed.kdl`. The inner `Err` is the
/// file, when one was reached, and why the write failed; the outer one is an unavailable
/// session.
///
/// A session without a config dir (one started with only `--config <file>`) saves into the
/// conventional config dir, creating the home one when none exists: that is where the loader
/// and the watcher look for `config.d`, so the save survives a restart. The folder beside the
/// `--config` file is never read for drop-ins.
fn write_managed_config_fragment(
    session_data: &Arc<RwLock<Option<SessionMetaData>>>,
    fragment: &str,
) -> Result<std::result::Result<ManagedConfigRevision, (Option<PathBuf>, String)>> {
    let config_dir = with_session(session_data, |session| session.config_dir.clone())?.flatten();
    Ok(write_fragment_in_config_dir(
        config_dir,
        yinz_runtime::home::find_or_create_default_config_dir,
        fragment,
    ))
}

/// Shown, cut to one line, by the plugin that asked for the save, so it says what to do.
const NO_CONFIG_DIR_REASON: &str = "no config directory; start Yinz with --config-dir";

/// Merge `fragment` into `config.d/yinz-managed.kdl` of `config_dir`, or of `default_dir()`
/// when the session has none. `default_dir` is only called then.
fn write_fragment_in_config_dir(
    config_dir: Option<PathBuf>,
    default_dir: impl FnOnce() -> Option<PathBuf>,
    fragment: &str,
) -> std::result::Result<ManagedConfigRevision, (Option<PathBuf>, String)> {
    match yinz_runtime::home::get_config_overlay_dir(config_dir.or_else(default_dir)) {
        Some(dir) => Config::write_managed_fragment_with_revision(&dir, fragment)
            .map(|(_, revision)| revision)
            .map_err(|e| {
                log::error!("Failed to persist the config fragment: {}", e);
                (
                    Some(dir.join(yinz_config::config::MANAGED_CONFIG_FILE_NAME)),
                    e.to_string(),
                )
            }),
        None => {
            log::error!("Failed to persist the config fragment: {NO_CONFIG_DIR_REASON}");
            Err((None, NO_CONFIG_DIR_REASON.to_owned()))
        },
    }
}

fn save_startup_axes(
    fragment: &str,
    session_data: &Arc<RwLock<Option<SessionMetaData>>>,
) -> Result<()> {
    match write_managed_config_fragment(session_data, fragment)? {
        Ok(revision) => {
            with_session_mut(session_data, |session| {
                session.session_configuration.startup_axes_save = Some(revision);
            })?;
            Ok(())
        },
        Err((path, reason)) => {
            with_session(session_data, |session| {
                session.report_config_write_failure(path, Some(reason.clone()))
            })?;
            Err(anyhow!("{reason}"))
        },
    }
}

fn update_new_saved_config(
    new_config: Option<Config>,
    persistence: ConfigPersistence,
    runtime_config_changed: bool,
    session_data: &Arc<RwLock<Option<SessionMetaData>>>,
    client_id: ClientId,
    deferred: Option<SessionConfiguration>,
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
            // A saved fragment is session-wide. Only the fragment reaches the session theme,
            // never the writing client's whole config with its own attach options; it is
            // committed only once the write succeeds.
            let source = with_session(session_data, |session| {
                session
                    .session_configuration
                    .application_theme_source_with(&fragment)
            })?
            .context("session is unavailable")??;
            let written = write_managed_config_fragment(session_data, &fragment)?;
            match written {
                Ok(_) => {
                    // The session's saved config is the running one now, and every client
                    // hears that it was written.
                    with_session_mut(session_data, |session| {
                        if let Some(staged) = deferred {
                            session.session_configuration = staged;
                        }
                        session
                            .session_configuration
                            .set_application_theme_source(source);
                        propagate_saved(session, new_config)
                    })?;
                },
                Err((file_path, reason)) => {
                    with_session(session_data, |session| {
                        session.report_config_write_failure(file_path, Some(reason.clone()))
                    })?;
                    if runtime_config_changed && deferred.is_none() {
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
        if runtime_config_changed {
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
