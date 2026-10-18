//! Shared layout and plugin launch models.
use super::{command::RunCommand, plugins::PluginAliases};
use crate::data::{OverlayPlacement, PluginTag};
use crate::geometry::{PercentOrFixed, SplitDirection, SplitSize};
use crate::internal_ids::TabPosition;
use crate::pane_size::{Dimension, PaneGeom, MIN_TERMINAL_HEIGHT, MIN_TERMINAL_WIDTH};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fmt::{self, Display, Formatter},
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    str::FromStr,
};
use url::Url;
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RunPluginOrAlias {
    RunPlugin(RunPlugin),
    Alias(PluginAlias),
}

impl Default for RunPluginOrAlias {
    fn default() -> Self {
        RunPluginOrAlias::RunPlugin(Default::default())
    }
}

impl RunPluginOrAlias {
    pub fn location_string(&self) -> String {
        match self {
            RunPluginOrAlias::RunPlugin(run_plugin) => run_plugin.location.display(),
            RunPluginOrAlias::Alias(plugin_alias) => plugin_alias.name.clone(),
        }
    }
    pub fn location_for_serialization(&self) -> String {
        match self {
            RunPluginOrAlias::RunPlugin(plugin) => plugin.location_for_serialization(),
            RunPluginOrAlias::Alias(alias) => alias.name.clone(),
        }
    }
    pub fn populate_run_plugin_if_needed(&mut self, plugin_aliases: &PluginAliases) {
        if let RunPluginOrAlias::Alias(run_plugin_alias) = self {
            let merged_run_plugin = plugin_aliases
                .aliases
                .get(run_plugin_alias.name.as_str())
                .map(|r| {
                    let mut merged_run_plugin = r.clone().merge_configuration(
                        &run_plugin_alias
                            .configuration
                            .as_ref()
                            .map(|c| c.inner().clone()),
                    );
                    // if the alias has its own cwd, it should always override the alias
                    // value's cwd
                    if run_plugin_alias.initial_cwd.is_some() {
                        merged_run_plugin.initial_cwd = run_plugin_alias.initial_cwd.clone();
                    }
                    merged_run_plugin
                });
            run_plugin_alias.run_plugin = merged_run_plugin;
        }
    }
    pub fn get_run_plugin(&self) -> Option<RunPlugin> {
        match self {
            RunPluginOrAlias::RunPlugin(run_plugin) => Some(run_plugin.clone()),
            RunPluginOrAlias::Alias(plugin_alias) => plugin_alias.run_plugin.clone(),
        }
    }
    pub fn get_configuration(&self) -> Option<PluginUserConfiguration> {
        self.get_run_plugin().map(|r| r.configuration.clone())
    }
    fn configuration_value_is_true(&self, key: &str) -> bool {
        let configuration = match self {
            RunPluginOrAlias::RunPlugin(run_plugin) => Some(&run_plugin.configuration),
            RunPluginOrAlias::Alias(alias) => alias
                .run_plugin
                .as_ref()
                .map(|run_plugin| &run_plugin.configuration)
                .or(alias.configuration.as_ref()),
        };
        configuration.is_some_and(|configuration| {
            configuration
                .inner()
                .get(key)
                .is_some_and(|value| value == "true")
        })
    }
    /// The overlay placement this plugin's own configuration asks for, if any.
    ///
    /// **Compatibility shim.** Overlay-ness is a *placement*
    /// (`NewPanePlacement::Overlay`), not a property of the plugin, but it used to be spelled as
    /// `overlay`/`modal` keys in the free-form `PluginUserConfiguration` string map - which is
    /// what every keybinding, layout and third-party plugin written so far still writes. This is
    /// the one place that spelling is understood; everything downstream takes the resolved
    /// `OverlayPlacement`. It can go once the old keys are no longer in circulation.
    pub fn overlay_placement(&self) -> Option<OverlayPlacement> {
        if self.configuration_value_is_true("modal") {
            Some(OverlayPlacement::modal(None))
        } else if self.configuration_value_is_true("overlay") {
            Some(OverlayPlacement::dismissible(None))
        } else {
            None
        }
    }
    /// Whether this plugin places its own overlay, and so must not be painted at the default
    /// geometry it is born with while it gets round to it.
    ///
    /// An overlay that named no coordinates is born half the surface, *centered*
    /// (`OverlayPanes::base_geom`) - which, for a pair of panels that belong in the bottom-left
    /// corner, is squarely on top of each other. The pane is only moved out of there by its owner,
    /// a round trip later, so the launch is visible as a flash in the middle of the screen before
    /// the panel appears where it belongs. Opting in here says "do not paint me until somebody has
    /// placed me"; the first `change_overlay_coordinates` naming the pane is what releases it.
    ///
    /// The self-sizing agent dialogs and quit confirmation opt in automatically, including
    /// resolved aliases. Other plugins opt in with `awaits_placement true`; overlays that do
    /// not place themselves can still appear immediately.
    ///
    /// **The key is part of plugin identity**, as `overlay` is - a plugin is (location,
    /// configuration) - so a keybinding that writes it and a plugin-side launch that does not
    /// address two different instances. See `agent_proto::launch::overlay_configuration`.
    pub fn awaits_placement(&self) -> bool {
        self.configuration_value_is_true("awaits_placement")
            || self.get_run_plugin().is_some_and(|plugin| {
                matches!(plugin.location, RunPluginLocation::Yinz(tag)
                    if matches!(tag.to_string().as_str(), "new-agent" | "quit-confirmation"))
            })
    }
    pub fn requests_overlay(&self) -> bool {
        self.overlay_placement().is_some()
    }
    pub fn requests_modal(&self) -> bool {
        self.overlay_placement().is_some_and(|p| p.is_modal())
    }
    pub fn get_initial_cwd(&self) -> Option<PathBuf> {
        match self {
            RunPluginOrAlias::RunPlugin(plugin) => plugin.initial_cwd.clone(),
            RunPluginOrAlias::Alias(alias) => alias.initial_cwd.clone().or_else(|| {
                alias
                    .run_plugin
                    .as_ref()
                    .and_then(|plugin| plugin.initial_cwd.clone())
            }),
        }
    }

    pub fn is_equivalent_to_run(&self, run: &Option<Run>) -> bool {
        let equivalent = |requested: &RunPlugin, running: &RunPlugin| {
            requested.configuration == running.configuration
                && requested.location == running.location
        };
        match (self, run) {
            (
                RunPluginOrAlias::Alias(self_alias),
                Some(Run::Plugin(RunPluginOrAlias::Alias(run_alias))),
            ) => {
                let without_caller_context = |configuration: Option<&PluginUserConfiguration>| {
                    configuration
                        .map(PluginUserConfiguration::without_caller_context)
                        .unwrap_or_default()
                };
                self_alias.name == run_alias.name
                    && without_caller_context(self_alias.configuration.as_ref())
                        == without_caller_context(run_alias.configuration.as_ref())
            },
            (
                RunPluginOrAlias::Alias(self_alias),
                Some(Run::Plugin(RunPluginOrAlias::RunPlugin(other_run_plugin))),
            ) => self_alias
                .run_plugin
                .as_ref()
                .is_some_and(|plugin| equivalent(plugin, other_run_plugin)),
            (
                RunPluginOrAlias::RunPlugin(self_run_plugin),
                Some(Run::Plugin(RunPluginOrAlias::RunPlugin(other_run_plugin))),
            ) => equivalent(self_run_plugin, other_run_plugin),
            _ => false,
        }
    }
    pub fn with_initial_cwd(mut self, initial_cwd: Option<PathBuf>) -> Self {
        match self {
            RunPluginOrAlias::RunPlugin(ref mut run_plugin) => {
                run_plugin.initial_cwd = initial_cwd;
            },
            RunPluginOrAlias::Alias(ref mut alias) => {
                alias.initial_cwd = initial_cwd;
            },
        }
        self
    }
    pub fn add_initial_cwd(&mut self, initial_cwd: &Path) {
        match self {
            RunPluginOrAlias::RunPlugin(ref mut run_plugin) => {
                run_plugin.initial_cwd = Some(initial_cwd.to_path_buf());
            },
            RunPluginOrAlias::Alias(ref mut alias) => {
                alias.initial_cwd = Some(initial_cwd.to_path_buf());
            },
        }
    }
    pub fn is_builtin_plugin(&self) -> bool {
        self.get_run_plugin()
            .map(|r| matches!(r.location, RunPluginLocation::Yinz(_)))
            .unwrap_or(false)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub enum Run {
    #[serde(rename = "plugin")]
    Plugin(RunPluginOrAlias),
    #[serde(rename = "command")]
    Command(RunCommand),
    EditFile(PathBuf, Option<usize>, Option<PathBuf>),
    Cwd(PathBuf),
}

impl Run {
    pub fn merge(base: &Option<Run>, other: &Option<Run>) -> Option<Run> {
        // Merges a pane_template's run (`base`) with its consumer's (`other`). Every
        // pair is spelled out: a new `Run` variant has to say what it inherits, rather
        // than silently replacing the template's run the way a catch-all did.
        match (base, other) {
            (Some(Run::Command(base_run_command)), Some(Run::Command(other_run_command))) => {
                let mut merged = other_run_command.clone();
                if merged.cwd.is_none() && base_run_command.cwd.is_some() {
                    merged.cwd = base_run_command.cwd.clone();
                }
                if merged.args.is_empty() && !base_run_command.args.is_empty() {
                    merged.args = base_run_command.args.clone();
                }
                merged.target.merge_from(&base_run_command.target);
                // A target-only consumer runs the template's command in that target.
                if merged.command.as_os_str().is_empty()
                    && (merged.remote_default_shell || merged.target.is_bubblewrap())
                    && !base_run_command.command.as_os_str().is_empty()
                {
                    merged.command = base_run_command.command.clone();
                    merged.remote_default_shell = false;
                    merged.hold_on_close = base_run_command.hold_on_close;
                }
                Some(Run::Command(merged))
            },
            (Some(Run::Command(base_run_command)), Some(Run::Cwd(other_cwd))) => {
                let mut merged = base_run_command.clone();
                merged.cwd = Some(other_cwd.clone());
                Some(Run::Command(merged))
            },
            (Some(Run::Cwd(base_cwd)), Some(Run::Command(other_command))) => {
                let mut merged = other_command.clone();
                if merged.cwd.is_none() {
                    merged.cwd = Some(base_cwd.clone());
                }
                Some(Run::Command(merged))
            },
            (
                Some(Run::Command(base_run_command)),
                Some(Run::EditFile(file_to_edit, line_number, edit_cwd)),
            ) => match &base_run_command.cwd {
                Some(cwd) => Some(Run::EditFile(
                    cwd.join(file_to_edit),
                    *line_number,
                    Some(cwd.join(edit_cwd.clone().unwrap_or_default())),
                )),
                None => Some(Run::EditFile(
                    file_to_edit.clone(),
                    *line_number,
                    edit_cwd.clone(),
                )),
            },
            (Some(Run::Cwd(cwd)), Some(Run::EditFile(file_to_edit, line_number, edit_cwd))) => {
                let cwd = edit_cwd.clone().unwrap_or(cwd.clone());
                Some(Run::EditFile(
                    cwd.join(file_to_edit),
                    *line_number,
                    Some(cwd),
                ))
            },
            (Some(Run::Plugin(base_plugin)), Some(Run::Cwd(other_cwd))) => {
                let mut merged = base_plugin.clone();
                merged.add_initial_cwd(other_cwd);
                Some(Run::Plugin(merged))
            },
            (Some(Run::Cwd(base_cwd)), Some(Run::Plugin(other_plugin))) => {
                let mut merged = other_plugin.clone();
                if merged.get_initial_cwd().is_none() {
                    merged.add_initial_cwd(base_cwd);
                }
                Some(Run::Plugin(merged))
            },
            (
                Some(Run::EditFile(file_to_edit, line_number, edit_cwd)),
                Some(Run::Cwd(other_cwd)),
            ) => {
                // The parser anchored the template's `edit` to the template's own cwd
                // (`parse_pane_command`); the consumer's cwd replaces that anchor, as it
                // replaces a command template's cwd. An `edit` written as an absolute
                // path stays where it points - unless it lies under the template's own
                // cwd, where it cannot be told from a relative one and follows the
                // consumer too.
                let file_to_edit = edit_cwd
                    .as_ref()
                    .and_then(|cwd| file_to_edit.strip_prefix(cwd).ok())
                    .unwrap_or(file_to_edit);
                Some(Run::EditFile(
                    other_cwd.join(file_to_edit),
                    *line_number,
                    Some(other_cwd.clone()),
                ))
            },
            // The consumer names a run of its own, so it replaces the template's.
            (
                Some(Run::Plugin(_) | Run::Command(_) | Run::EditFile(..)),
                Some(other @ Run::Plugin(_)),
            )
            | (Some(Run::Plugin(_) | Run::EditFile(..)), Some(other @ Run::Command(_)))
            | (Some(Run::Plugin(_) | Run::EditFile(..)), Some(other @ Run::EditFile(..)))
            | (Some(Run::Cwd(_)), Some(other @ Run::Cwd(_))) => Some(other.clone()),
            (Some(base), None) => Some(base.clone()),
            (None, Some(other)) => Some(other.clone()),
            (None, None) => None,
        }
    }
    /// Prefix a layout, tab or template `cwd` onto this pane's own.
    ///
    pub fn add_cwd(&mut self, cwd: &Path) {
        match self {
            Run::Command(run_command) => match run_command.cwd.as_mut() {
                Some(run_cwd) => {
                    *run_cwd = cwd.join(&run_cwd);
                },
                None => {
                    run_command.cwd = Some(cwd.to_path_buf());
                },
            },
            Run::EditFile(path_to_file, _line_number, edit_cwd) => {
                match edit_cwd.as_mut() {
                    Some(edit_cwd) => {
                        *edit_cwd = cwd.join(&edit_cwd);
                    },
                    None => {
                        let _ = edit_cwd.insert(cwd.to_path_buf());
                    },
                };
                *path_to_file = cwd.join(&path_to_file);
            },
            Run::Cwd(path) => {
                *path = cwd.join(&path);
            },
            Run::Plugin(run_plugin_or_alias) => {
                run_plugin_or_alias.add_initial_cwd(cwd);
            },
        }
    }
    pub fn add_args(&mut self, args: Option<Vec<String>>) {
        // overrides the args of a Run::Command if they are Some
        // and not empty
        if let Some(args) = args {
            if let Run::Command(run_command) = self {
                if !args.is_empty() {
                    run_command.args = args.clone();
                }
            }
        }
    }
    pub fn add_close_on_exit(&mut self, close_on_exit: Option<bool>) {
        // overrides the hold_on_close of a Run::Command if it is Some
        // and not empty
        if let Some(close_on_exit) = close_on_exit {
            if let Run::Command(run_command) = self {
                run_command.hold_on_close = !close_on_exit;
            }
        }
    }
    pub fn add_start_suspended(&mut self, start_suspended: Option<bool>) {
        // overrides the hold_on_start of a Run::Command if they are Some
        // and not empty
        if let Some(start_suspended) = start_suspended {
            if let Run::Command(run_command) = self {
                run_command.hold_on_start = start_suspended;
            }
        }
    }
    pub fn is_same_category(first: &Option<Run>, second: &Option<Run>) -> bool {
        matches!(
            (first, second),
            (Some(Run::Plugin(..)), Some(Run::Plugin(..)))
                | (Some(Run::Command(..)), Some(Run::Command(..)))
                | (Some(Run::EditFile(..)), Some(Run::EditFile(..)))
                | (Some(Run::Cwd(..)), Some(Run::Cwd(..)))
        )
    }
    pub fn is_terminal(run: &Option<Run>) -> bool {
        matches!(
            run,
            Some(Run::Command(..)) | Some(Run::EditFile(..)) | Some(Run::Cwd(..)) | None
        )
    }
    pub fn get_cwd(&self) -> Option<PathBuf> {
        match self {
            Run::Plugin(_) => None, // TBD
            Run::Command(run_command) => run_command.cwd.clone(),
            Run::EditFile(_file, _line_num, cwd) => cwd.clone(),
            Run::Cwd(cwd) => Some(cwd.clone()),
        }
    }
    pub fn get_run_plugin(&self) -> Option<RunPlugin> {
        match self {
            Run::Plugin(RunPluginOrAlias::RunPlugin(run_plugin)) => Some(run_plugin.clone()),
            Run::Plugin(RunPluginOrAlias::Alias(plugin_alias)) => plugin_alias.run_plugin.clone(),
            _ => None,
        }
    }
    pub fn populate_run_plugin_if_needed(&mut self, alias_dict: &PluginAliases) {
        if let Run::Plugin(run_plugin_alias) = self {
            run_plugin_alias.populate_run_plugin_if_needed(alias_dict)
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct RunPlugin {
    #[serde(default)]
    pub _allow_exec_host_cmd: bool,
    pub location: RunPluginLocation,
    pub configuration: PluginUserConfiguration,
    pub initial_cwd: Option<PathBuf>,
    /// The path as the user wrote it, when the URL this was built from was a relative `file:`
    /// one - see `yinz_runtime::plugin_location::parse_plugin_location_with_relative`. Not part of the plugin's identity:
    /// `$cwd/keys.ts` is the same plugin however it was named.
    #[serde(default)]
    pub relative_path: Option<PathBuf>,
    /// Loading path before canonicalization, so reload follows a retargeted symlink.
    /// Canonical `location` alone determines file identity.
    #[serde(default)]
    pub declared_path: Option<PathBuf>,
}

impl RunPlugin {
    /// Keep plugin-directory fallback when the cwd-resolved file is confirmed missing.
    pub fn location_for_serialization(&self) -> String {
        if let RunPluginLocation::File(path) = &self.location {
            let path = self.declared_path.as_ref().unwrap_or(path);
            if let Some(relative_path) = &self.relative_path {
                if matches!(path.try_exists(), Ok(false)) {
                    return format!("file:{}", relative_path.display());
                }
            }
            return Url::from_file_path(path)
                .map(|url| url.to_string())
                .unwrap_or_else(|_| format!("file:{}", path.display()));
        }
        self.location.display()
    }

    pub fn with_configuration(mut self, configuration: BTreeMap<String, String>) -> Self {
        self.configuration = PluginUserConfiguration::new(configuration);
        self
    }
    pub fn with_initial_cwd(mut self, initial_cwd: Option<PathBuf>) -> Self {
        self.initial_cwd = initial_cwd;
        self
    }
    pub fn merge_configuration(mut self, configuration: &Option<BTreeMap<String, String>>) -> Self {
        if let Some(configuration) = configuration {
            self.configuration.merge(configuration);
        }
        self
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, Eq)]
pub struct PluginAlias {
    pub name: String,
    pub configuration: Option<PluginUserConfiguration>,
    pub initial_cwd: Option<PathBuf>,
    pub run_plugin: Option<RunPlugin>,
}

impl PartialEq for PluginAlias {
    fn eq(&self, other: &Self) -> bool {
        // NOTE: Keep this in sync with what the `Hash` trait impl does.
        self.name == other.name && self.configuration == other.configuration
    }
}

impl std::hash::Hash for PluginAlias {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        // NOTE: Keep this in sync with what the `PartiqlEq` trait impl does.
        self.name.hash(state);
        self.configuration.hash(state);
    }
}

impl Ord for PluginAlias {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // NOTE: Keep this in sync with what the `PartialEq` and `Hash` impls do.
        (&self.name, &self.configuration).cmp(&(&other.name, &other.configuration))
    }
}

impl PartialOrd for PluginAlias {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl PluginAlias {
    pub fn new(
        name: &str,
        configuration: &Option<BTreeMap<String, String>>,
        initial_cwd: Option<PathBuf>,
    ) -> Self {
        PluginAlias {
            name: name.to_owned(),
            configuration: configuration
                .as_ref()
                .map(|c| PluginUserConfiguration::new(c.clone())),
            initial_cwd,
            ..Default::default()
        }
    }
    pub fn set_caller_cwd_if_not_set(&mut self, caller_cwd: Option<PathBuf>) {
        // we do this only for an alias because in all other cases this will be handled by the
        // "cwd" configuration key above
        // for an alias we might have cases where the cwd is defined on the alias but we still
        // want to pass the "caller" cwd for the plugin the alias resolves into (eg. a
        // filepicker that has access to the whole filesystem but wants to start in a specific
        // folder)
        if let Some(caller_cwd) = caller_cwd {
            if self
                .configuration
                .as_ref()
                .map(|c| c.inner().get("caller_cwd").is_none())
                .unwrap_or(true)
            {
                let configuration = self
                    .configuration
                    .get_or_insert_with(|| PluginUserConfiguration::new(BTreeMap::new()));
                configuration.insert("caller_cwd", caller_cwd.display().to_string());
            }
        }
    }
    pub fn set_caller_terminal_pane_id_if_not_set(
        &mut self,
        pane_id: Option<crate::internal_ids::TerminalId>,
    ) {
        if let Some(pane_id) = pane_id {
            let configuration = self
                .configuration
                .get_or_insert_with(|| PluginUserConfiguration::new(BTreeMap::new()));
            if configuration
                .inner()
                .get("caller_terminal_pane_id")
                .is_none()
            {
                configuration.insert("caller_terminal_pane_id", pane_id.to_string());
            }
        }
    }
}

#[allow(clippy::derived_hash_with_manual_eq)]
impl PartialEq for RunPlugin {
    fn eq(&self, other: &Self) -> bool {
        (&self.location, &self.configuration) == (&other.location, &other.configuration)
    }
}
impl Eq for RunPlugin {}

impl Hash for RunPlugin {
    // NOTE: Keep this in sync with what the `PartialEq` and `Ord` impls do. It used to be
    // derived, which hashed the fields equality ignores - so two plugins that compare equal
    // could land in different buckets, and a `HashSet<RunPlugin>` (`pending_plugin_reloads`)
    // could hold both.
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.location.hash(state);
        self.configuration.hash(state);
    }
}

impl Ord for RunPlugin {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // NOTE: Keep this in sync with what the `PartialEq` and `Hash` impls do:
        // ordering has to agree with equality, so it compares the same fields
        // and deliberately ignores loading paths, `initial_cwd` and `_allow_exec_host_cmd`.
        (&self.location, &self.configuration).cmp(&(&other.location, &other.configuration))
    }
}

impl PartialOrd for RunPlugin {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PluginUserConfiguration(BTreeMap<String, String>);

impl PluginUserConfiguration {
    pub fn new(mut configuration: BTreeMap<String, String>) -> Self {
        // reserved words
        configuration.remove("hold_on_close");
        configuration.remove("hold_on_start");
        configuration.remove("cwd");
        configuration.remove("name");
        configuration.remove("direction");
        configuration.remove("floating");
        configuration.remove("move_to_focused_tab");
        configuration.remove("launch_new");
        configuration.remove("payload");
        configuration.remove("skip_cache");
        configuration.remove("title");
        configuration.remove("in_place");
        configuration.remove("skip_plugin_cache");

        PluginUserConfiguration(configuration)
    }
    pub fn inner(&self) -> &BTreeMap<String, String> {
        &self.0
    }
    pub fn into_inner(self) -> BTreeMap<String, String> {
        self.0
    }
    /// This configuration with the per-launch caller context taken out of it.
    ///
    /// `caller_cwd` and `caller_terminal_pane_id` are stamped onto a launch by whoever asked for
    /// it (`Pty::spawn_plugin`, `yinz_exports::message_to_plugin`) rather than written by the
    /// user, so two launches of the same plugin from two different places carry different values
    /// for them. They are therefore *not* part of a plugin's identity, and every comparison that
    /// asks "is this the plugin that is already running?" has to look past them - otherwise a
    /// plugin asking for its other half by url opens a second copy of a pane that is already on
    /// screen, which is how agent ended up with two conversations and an agent list placed by the
    /// wrong one.
    ///
    /// Here rather than copied into each caller: it was three separate closures, one of which
    /// (the plugin map lookup) was a plain equality that had not been given the treatment at all.
    pub fn without_caller_context(&self) -> BTreeMap<String, String> {
        let mut configuration = self.0.clone();
        configuration.remove("caller_cwd");
        configuration.remove("caller_terminal_pane_id");
        configuration
    }
    pub fn insert(&mut self, config_key: impl Into<String>, config_value: impl Into<String>) {
        self.0.insert(config_key.into(), config_value.into());
    }
    pub fn merge(&mut self, other_config: &BTreeMap<String, String>) {
        for (key, value) in other_config {
            self.0.insert(key.to_owned(), value.clone());
        }
    }
}

impl FromStr for PluginUserConfiguration {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut ret = BTreeMap::new();
        let configs = s.split(',');
        for config in configs {
            let mut config = config.split('=');
            let key = config.next().ok_or("invalid configuration key")?.to_owned();
            let value = config.map(|c| c.to_owned()).collect::<Vec<_>>().join("=");
            ret.insert(key, value);
        }
        Ok(PluginUserConfiguration(ret))
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RunPluginLocation {
    File(PathBuf),
    Yinz(PluginTag),
    Git(super::git_plugin::GitPlugin),
}

impl Default for RunPluginLocation {
    fn default() -> Self {
        RunPluginLocation::File(Default::default())
    }
}

impl RunPluginLocation {
    pub fn display(&self) -> String {
        match self {
            RunPluginLocation::File(pathbuf) => format!("file:{}", pathbuf.display()),
            RunPluginLocation::Yinz(plugin_tag) => format!("yinz:{}", plugin_tag),
            RunPluginLocation::Git(plugin) => plugin.descriptor(),
        }
    }
}

impl From<&RunPluginLocation> for Url {
    fn from(location: &RunPluginLocation) -> Self {
        let url = match location {
            RunPluginLocation::File(path) => format!(
                "file:{}",
                path.clone().into_os_string().into_string().unwrap()
            ),
            RunPluginLocation::Yinz(tag) => format!("yinz:{}", tag),
            RunPluginLocation::Git(plugin) => plugin.descriptor(),
        };
        Self::parse(&url).unwrap()
    }
}

impl fmt::Display for RunPluginLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        match self {
            Self::File(path) => write!(
                f,
                "{}",
                path.clone().into_os_string().into_string().unwrap()
            ),
            Self::Yinz(tag) => write!(f, "{}", tag),
            Self::Git(plugin) => write!(f, "{}", plugin.descriptor()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub enum LayoutConstraint {
    MaxPanes(usize),
    MinPanes(usize),
    ExactPanes(usize),
    NoConstraint,
}

impl Display for LayoutConstraint {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        match self {
            LayoutConstraint::MaxPanes(max_panes) => write!(f, "max_panes={}", max_panes),
            LayoutConstraint::MinPanes(min_panes) => write!(f, "min_panes={}", min_panes),
            LayoutConstraint::ExactPanes(exact_panes) => write!(f, "exact_panes={}", exact_panes),
            LayoutConstraint::NoConstraint => write!(f, ""),
        }
    }
}

pub type SwapTiledLayout = (BTreeMap<LayoutConstraint, TiledPaneLayout>, Option<String>); // Option<String> is the swap layout name
pub type SwapFloatingLayout = (
    BTreeMap<LayoutConstraint, Vec<FloatingPaneLayout>>,
    Option<String>,
); // Option<String> is the swap layout name

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct Layout {
    pub tabs: Vec<(Option<String>, TiledPaneLayout, Vec<FloatingPaneLayout>)>,
    pub focused_tab_index: Option<TabPosition>,
    pub template: Option<(TiledPaneLayout, Vec<FloatingPaneLayout>)>,
    pub swap_layouts: Vec<(TiledPaneLayout, Vec<FloatingPaneLayout>)>,
    pub swap_tiled_layouts: Vec<SwapTiledLayout>,
    pub swap_floating_layouts: Vec<SwapFloatingLayout>,
}

/// Layout configuration for a single tab in multi-tab override
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct TabLayoutInfo {
    /// Zero-based display position, resolved to a stable ID by the screen.
    pub tab_index: TabPosition,
    pub tab_name: Option<String>,
    pub tiled_layout: TiledPaneLayout,
    pub floating_layouts: Vec<FloatingPaneLayout>,
    pub swap_tiled_layouts: Option<Vec<SwapTiledLayout>>,
    pub swap_floating_layouts: Option<Vec<SwapFloatingLayout>>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct FloatingPaneLayout {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_tabs: Option<crate::pane_tabs::PaneTabsLayout>,
    pub name: Option<String>,
    pub height: Option<PercentOrFixed>,
    pub width: Option<PercentOrFixed>,
    pub x: Option<PercentOrFixed>,
    pub y: Option<PercentOrFixed>,
    pub pinned: Option<bool>,
    pub borderless: Option<bool>,
    pub run: Option<Run>,
    pub focus: Option<bool>,
    pub already_running: bool,
    pub pane_initial_contents: Option<String>,
    pub logical_position: Option<usize>,
    pub default_fg: Option<String>,
    pub default_bg: Option<String>,
}

impl FloatingPaneLayout {
    pub fn populate_plugin_aliases(&mut self, aliases: &PluginAliases) {
        if let Some(run) = &mut self.run {
            run.populate_run_plugin_if_needed(aliases);
        }
        if let Some(tabs) = &mut self.pane_tabs {
            for descriptor in &mut tabs.tabs {
                descriptor.populate_plugin_alias(aliases);
            }
        }
    }

    pub fn new() -> Self {
        FloatingPaneLayout {
            pane_tabs: None,
            name: None,
            height: None,
            width: None,
            x: None,
            y: None,
            pinned: None,
            borderless: None,
            run: None,
            focus: None,
            already_running: false,
            pane_initial_contents: None,
            logical_position: None,
            default_fg: None,
            default_bg: None,
        }
    }
    pub fn add_cwd_to_layout(&mut self, cwd: &Path) {
        if let Some(tabs) = &mut self.pane_tabs {
            for tab in &mut tabs.tabs {
                tab.add_cwd(cwd);
            }
        }
        match self.run.as_mut() {
            Some(run) => run.add_cwd(cwd),
            None => {
                self.run = Some(Run::Cwd(cwd.to_path_buf()));
            },
        }
        if let (Some(tabs), Some(Run::Plugin(plugin))) = (&self.pane_tabs, &mut self.run) {
            if let Some(selected) = tabs.tabs.get(tabs.active) {
                *plugin = plugin
                    .clone()
                    .with_initial_cwd(selected.initial_cwd.clone());
            }
        }
    }
    pub fn add_start_suspended(&mut self, start_suspended: Option<bool>) {
        if let Some(run) = self.run.as_mut() {
            run.add_start_suspended(start_suspended);
        }
    }
}

impl From<&TiledPaneLayout> for FloatingPaneLayout {
    fn from(pane_layout: &TiledPaneLayout) -> Self {
        FloatingPaneLayout {
            pane_tabs: pane_layout.pane_tabs.clone(),
            name: pane_layout.name.clone(),
            run: pane_layout.run.clone(),
            focus: pane_layout.focus,
            ..Default::default()
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct TiledPaneLayout {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_tabs: Option<crate::pane_tabs::PaneTabsLayout>,
    pub children_split_direction: SplitDirection,
    pub name: Option<String>,
    pub children: Vec<TiledPaneLayout>,
    pub split_size: Option<SplitSize>,
    pub run: Option<Run>,
    pub borderless: Option<bool>,
    pub focus: Option<bool>,
    pub external_children_index: Option<usize>,
    pub children_are_stacked: bool,
    pub is_expanded_in_stack: bool,
    pub exclude_from_sync: Option<bool>,
    pub run_instructions_to_ignore: Vec<Option<Run>>,
    pub hide_floating_panes: bool, // only relevant if this is the base layout
    /// Whether the user chose the name of the tab this is the base layout of, rather than the
    /// layout giving it one. Only a chosen name holds off the project the tab would otherwise
    /// show. Set by a name typed in a command, and by a resurrected session for a rename.
    #[serde(default)]
    pub user_named: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_group: Option<crate::tab_bar::TabGroupInfo>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub group_focused: bool,
    pub pane_initial_contents: Option<String>,
    pub default_fg: Option<String>,
    pub default_bg: Option<String>,
}

impl TiledPaneLayout {
    pub fn insert_children_layout(&mut self, children_layout: &mut TiledPaneLayout) -> bool {
        // returns true if successfully inserted and false otherwise
        match self.external_children_index {
            Some(external_children_index) => {
                self.children
                    .insert(external_children_index, children_layout.clone());
                self.external_children_index = None;
                true
            },
            None => {
                for pane in self.children.iter_mut() {
                    if pane.insert_children_layout(children_layout) {
                        return true;
                    }
                }
                false
            },
        }
    }
    pub fn insert_children_nodes(&mut self, children_nodes: &mut Vec<TiledPaneLayout>) -> bool {
        // returns true if successfully inserted and false otherwise
        match self.external_children_index {
            Some(external_children_index) => {
                children_nodes.reverse();
                for child_node in children_nodes.drain(..) {
                    self.children.insert(external_children_index, child_node);
                }
                self.external_children_index = None;
                true
            },
            None => {
                for pane in self.children.iter_mut() {
                    if pane.insert_children_nodes(children_nodes) {
                        return true;
                    }
                }
                false
            },
        }
    }
    pub fn children_block_count(&self) -> usize {
        let mut count = 0;
        if self.external_children_index.is_some() {
            count += 1;
        }
        for pane in &self.children {
            count += pane.children_block_count();
        }
        count
    }
    pub fn pane_count(&self) -> usize {
        if self.children.is_empty() {
            1 // self
        } else {
            let mut pane_count = 0;
            for child in &self.children {
                pane_count += child.pane_count();
            }
            pane_count
        }
    }
    fn children_node_replaces_pane(&self) -> Option<bool> {
        if self.external_children_index.is_some() {
            Some(self.children.is_empty())
        } else {
            self.children
                .iter()
                .find_map(Self::children_node_replaces_pane)
        }
    }
    pub fn position_panes_in_space(
        &self,
        space: &PaneGeom,
        max_panes: Option<usize>,
        ignore_percent_split_sizes: bool,
        focus_layout_if_not_focused: bool,
    ) -> Result<Vec<(TiledPaneLayout, PaneGeom)>, LayoutError> {
        let layouts = match max_panes {
            Some(max_panes) => {
                let mut layout_to_split = self.clone();
                let pane_count_in_layout = layout_to_split.pane_count();
                if max_panes > pane_count_in_layout {
                    // A childless placeholder was counted as a pane and becomes a container.
                    // With siblings, its parent was already a container and needs no replacement.
                    let children_count = (max_panes - pane_count_in_layout)
                        + usize::from(
                            layout_to_split
                                .children_node_replaces_pane()
                                .unwrap_or(false),
                        );
                    let mut extra_children = vec![TiledPaneLayout::default(); children_count];
                    if !layout_to_split.has_focused_node() && focus_layout_if_not_focused {
                        if let Some(last_child) = extra_children.last_mut() {
                            last_child.focus = Some(true);
                        }
                    }
                    let _ = layout_to_split.insert_children_nodes(&mut extra_children);
                } else {
                    layout_to_split.truncate(max_panes);
                }
                if !layout_to_split.has_focused_node() && focus_layout_if_not_focused {
                    layout_to_split.focus_deepest_pane();
                }

                let mut stack_id = 0;
                split_space(
                    space,
                    &layout_to_split,
                    space,
                    ignore_percent_split_sizes,
                    &mut stack_id,
                )?
            },
            None => {
                let mut stack_id = 0;
                split_space(
                    space,
                    self,
                    space,
                    ignore_percent_split_sizes,
                    &mut stack_id,
                )?
            },
        };
        for (_pane_layout, pane_geom) in layouts.iter() {
            if !pane_geom.is_at_least_minimum_size()
                || (pane_geom.rows.is_percent() && pane_geom.rows.as_usize() < MIN_TERMINAL_HEIGHT)
                || (pane_geom.cols.is_percent() && pane_geom.cols.as_usize() < MIN_TERMINAL_WIDTH)
            {
                return Err(LayoutError::UndersizedPane);
            }
        }
        Ok(layouts)
    }
    pub fn extract_run_instructions(&self) -> Vec<Option<Run>> {
        // the order of these run instructions is significant and needs to be the same
        // as the order of the "flattened" layout panes received from eg. position_panes_in_space
        let mut run_instructions = vec![];
        if self.children.is_empty() {
            run_instructions.push(self.run.clone());
        }
        let mut run_instructions_of_children = vec![];
        for child in &self.children {
            let mut child_run_instructions = child.extract_run_instructions();
            // add the only first child to run_instructions only adding the others after all the
            // childfree panes have been added so that the returned vec will be sorted breadth-first
            if !child_run_instructions.is_empty() {
                run_instructions.push(child_run_instructions.remove(0));
            }
            run_instructions_of_children.append(&mut child_run_instructions);
        }
        run_instructions.append(&mut run_instructions_of_children);
        let mut successfully_ignored = 0;
        for instruction_to_ignore in &self.run_instructions_to_ignore {
            if let Some(position) = run_instructions
                .iter()
                .position(|i| i == instruction_to_ignore)
            {
                run_instructions.remove(position);
                successfully_ignored += 1;
            }
        }
        // we need to do this because if we have an ignored instruction that does not match any
        // running instruction, we'll have an extra pane and our state will be messed up and we'll
        // crash (this can happen for example when breaking a plugin pane into a new tab that does
        // not have room for it but has a terminal instead)
        if successfully_ignored < self.run_instructions_to_ignore.len() {
            for _ in 0..self
                .run_instructions_to_ignore
                .len()
                .saturating_sub(successfully_ignored)
            {
                if let Some(position) = run_instructions.iter().position(|i| {
                    match i {
                        // this is because a bare CWD instruction should be overidden by a terminal
                        // in run_instructions_to_ignore (for cases where the cwd for example comes
                        // from a global layout cwd and the pane is actually just a bare pane that
                        // wants to be overidden)
                        Some(Run::Cwd(_)) | None => true,
                        _ => false,
                    }
                }) {
                    run_instructions.remove(position);
                }
            }
        }
        run_instructions
    }
    pub fn replace_next_empty_slot_with_run(&mut self, run_to_insert: Run) -> bool {
        // Replaces the first empty slot (None or Run::Cwd) with the given Run instruction.
        // Returns true if a replacement was made, false if no empty slot was found.
        // Traversal order matches extract_run_instructions (breadth-first).

        if self.children.is_empty() {
            // This is a leaf node - check if it's an empty slot
            match &self.run {
                None | Some(Run::Cwd(_)) => {
                    self.run = Some(run_to_insert);
                    return true;
                },
                _ => return false,
            }
        }

        // Check first child of each child (breadth-first first level)
        for child in self.children.iter_mut() {
            if child.children.is_empty() {
                match &child.run {
                    None | Some(Run::Cwd(_)) => {
                        child.run = Some(run_to_insert);
                        return true;
                    },
                    _ => {},
                }
            }
        }

        // Recursively check deeper levels (breadth-first continuation)
        for child in self.children.iter_mut() {
            if child.replace_next_empty_slot_with_run(run_to_insert.clone()) {
                return true;
            }
        }

        false
    }
    pub fn ignore_run_instruction(&mut self, run_instruction: Option<Run>) {
        self.run_instructions_to_ignore.push(run_instruction);
    }
    pub fn with_one_pane() -> Self {
        TiledPaneLayout {
            children: vec![TiledPaneLayout::default()],
            ..Default::default()
        }
    }
    pub fn add_cwd_to_layout(&mut self, cwd: &Path) {
        if let Some(tabs) = &mut self.pane_tabs {
            for tab in &mut tabs.tabs {
                tab.add_cwd(cwd);
            }
        }
        match self.run.as_mut() {
            Some(run) => run.add_cwd(cwd),
            None => {
                self.run = Some(Run::Cwd(cwd.to_path_buf()));
            },
        }
        if let (Some(tabs), Some(Run::Plugin(plugin))) = (&self.pane_tabs, &mut self.run) {
            if let Some(selected) = tabs.tabs.get(tabs.active) {
                *plugin = plugin
                    .clone()
                    .with_initial_cwd(selected.initial_cwd.clone());
            }
        }
        for child in self.children.iter_mut() {
            child.add_cwd_to_layout(cwd);
        }
    }
    pub fn populate_plugin_aliases_in_layout(&mut self, plugin_aliases: &PluginAliases) {
        if let Some(tabs) = &mut self.pane_tabs {
            for descriptor in &mut tabs.tabs {
                descriptor.populate_plugin_alias(plugin_aliases);
            }
        }
        if let Some(run) = self.run.as_mut() {
            run.populate_run_plugin_if_needed(plugin_aliases)
        }
        for child in self.children.iter_mut() {
            child.populate_plugin_aliases_in_layout(plugin_aliases);
        }
    }
    pub fn deepest_depth(&self) -> usize {
        let mut deepest_child_depth = 0;
        for child in self.children.iter() {
            let child_deepest_depth = child.deepest_depth();
            if child_deepest_depth > deepest_child_depth {
                deepest_child_depth = child_deepest_depth;
            }
        }
        deepest_child_depth + 1
    }
    pub fn focus_deepest_pane(&mut self) {
        let mut deepest_child_index = None;
        let mut deepest_path = 0;
        for (i, child) in self.children.iter().enumerate() {
            let child_deepest_path = child.deepest_depth();
            if child_deepest_path >= deepest_path {
                deepest_path = child_deepest_path;
                deepest_child_index = Some(i)
            }
        }
        match deepest_child_index {
            Some(deepest_child_index) => {
                if let Some(child) = self.children.get_mut(deepest_child_index) {
                    child.focus_deepest_pane();
                }
            },
            None => {
                self.focus = Some(true);
            },
        }
    }
    pub fn truncate(&mut self, max_panes: usize) -> usize {
        // returns remaining children length
        // if max_panes is 1, it means there's only enough panes for this node,
        // if max_panes is 0, this is probably the root layout being called with 0 max panes
        if max_panes <= 1 {
            while !self.children.is_empty() {
                // this is a special case: we're truncating a pane that was previously a logical
                // container but now should be an actual pane - so here we'd like to use its
                // deepest "non-logical" child in order to get all its attributes (eg. borderless)
                let first_child = self.children.remove(0);
                drop(std::mem::replace(self, first_child));
            }
            self.children.clear();
        } else if max_panes <= self.children.len() {
            self.children.truncate(max_panes);
            self.children.iter_mut().for_each(|l| l.children.clear());
        } else {
            let mut remaining_panes = max_panes
                - self
                    .children
                    .iter()
                    .filter(|c| c.children.is_empty())
                    .count();
            for child in self.children.iter_mut() {
                if remaining_panes > 1 && !child.children.is_empty() {
                    remaining_panes =
                        remaining_panes.saturating_sub(child.truncate(remaining_panes));
                } else {
                    child.children.clear();
                }
            }
        }
        if !self.children.is_empty() {
            self.children.len()
        } else {
            1 // just me
        }
    }
    pub fn has_focused_node(&self) -> bool {
        if self.focus.unwrap_or(false) {
            return true;
        };
        for child in &self.children {
            if child.has_focused_node() {
                return true;
            }
        }
        false
    }
    pub fn recursively_add_start_suspended(&mut self, start_suspended: Option<bool>) {
        if let Some(run) = self.run.as_mut() {
            run.add_start_suspended(start_suspended);
        }
        for child in self.children.iter_mut() {
            child.recursively_add_start_suspended(start_suspended);
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub enum LayoutParts {
    Tabs(Vec<(Option<String>, Layout)>), // String is the tab name
    Panes(Vec<Layout>),
}

impl LayoutParts {
    pub fn is_empty(&self) -> bool {
        match self {
            LayoutParts::Panes(panes) => panes.is_empty(),
            LayoutParts::Tabs(tabs) => tabs.is_empty(),
        }
    }
    pub fn insert_pane(&mut self, index: usize, layout: Layout) -> Result<(), &'static str> {
        match self {
            LayoutParts::Panes(panes) => {
                panes.insert(index, layout);
                Ok(())
            },
            LayoutParts::Tabs(_tabs) => Err("Trying to insert a pane into a tab layout"),
        }
    }
}

impl Default for LayoutParts {
    fn default() -> Self {
        LayoutParts::Panes(vec![])
    }
}

impl Layout {
    pub fn new_tab(&self) -> (TiledPaneLayout, Vec<FloatingPaneLayout>) {
        self.template.clone().unwrap_or_default()
    }

    pub fn has_tabs(&self) -> bool {
        !self.tabs.is_empty()
    }

    pub fn tabs(&self) -> Vec<(Option<String>, TiledPaneLayout, Vec<FloatingPaneLayout>)> {
        // String is the tab name
        self.tabs.clone()
    }

    pub fn focused_tab_index(&self) -> Option<TabPosition> {
        self.focused_tab_index
    }

    pub fn recursively_add_start_suspended(&mut self, start_suspended: Option<bool>) {
        for (_tab_name, tiled_panes, floating_panes) in self.tabs.iter_mut() {
            tiled_panes.recursively_add_start_suspended(start_suspended);
            for floating_pane in floating_panes.iter_mut() {
                floating_pane.add_start_suspended(start_suspended);
            }
        }
    }
    pub fn recursively_add_start_suspended_including_template(
        &mut self,
        start_suspended: Option<bool>,
    ) {
        if let Some((tiled_panes_template, floating_panes_template)) = self.template.as_mut() {
            tiled_panes_template.recursively_add_start_suspended(start_suspended);
            for floating_pane in floating_panes_template.iter_mut() {
                floating_pane.add_start_suspended(start_suspended);
            }
        }
        for (_tab_name, tiled_panes, floating_panes) in self.tabs.iter_mut() {
            tiled_panes.recursively_add_start_suspended(start_suspended);
            for floating_pane in floating_panes.iter_mut() {
                floating_pane.add_start_suspended(start_suspended);
            }
        }
    }

    pub fn populate_plugin_aliases_in_layout(&mut self, plugin_aliases: &PluginAliases) {
        for tab in self.tabs.iter_mut() {
            tab.1.populate_plugin_aliases_in_layout(plugin_aliases);
            for floating_pane_layout in tab.2.iter_mut() {
                floating_pane_layout.populate_plugin_aliases(plugin_aliases);
            }
        }
        if let Some(template) = self.template.as_mut() {
            template.0.populate_plugin_aliases_in_layout(plugin_aliases);
            for floating_pane_layout in template.1.iter_mut() {
                floating_pane_layout.populate_plugin_aliases(plugin_aliases);
            }
        }
        for swap_tiled_layout in &mut self.swap_tiled_layouts {
            for tiled_pane_layout in swap_tiled_layout.0.values_mut() {
                tiled_pane_layout.populate_plugin_aliases_in_layout(plugin_aliases);
            }
        }
        for swap_floating_layout in &mut self.swap_floating_layouts {
            for floating_pane_layouts in swap_floating_layout.0.values_mut() {
                for floating_pane_layout in floating_pane_layouts {
                    floating_pane_layout.populate_plugin_aliases(plugin_aliases);
                }
            }
        }
    }
    pub fn add_cwd_to_layout(&mut self, cwd: &Path) {
        for (_, tiled_pane_layout, floating_panes) in self.tabs.iter_mut() {
            tiled_pane_layout.add_cwd_to_layout(cwd);
            for floating_pane in floating_panes {
                floating_pane.add_cwd_to_layout(cwd);
            }
        }
        if let Some((tiled_pane_layout, floating_panes)) = self.template.as_mut() {
            tiled_pane_layout.add_cwd_to_layout(cwd);
            for floating_pane in floating_panes {
                floating_pane.add_cwd_to_layout(cwd);
            }
        }
    }
    pub fn pane_count(&self) -> usize {
        let mut pane_count = 0;
        if let Some((tiled_pane_layout, floating_panes)) = self.template.as_ref() {
            pane_count += tiled_pane_layout.pane_count();
            for _ in floating_panes {
                pane_count += 1;
            }
        }
        for (_, tiled_pane_layout, floating_panes) in &self.tabs {
            pane_count += tiled_pane_layout.pane_count();
            for _ in floating_panes {
                pane_count += 1;
            }
        }
        pane_count
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LayoutError {
    #[error("Not enough room for panes")]
    InsufficientSplitSpace,
    #[error("Not enough room for stacked panes in this layout")]
    InsufficientStackSpace,
    #[error("No room on screen for this layout!")]
    UndersizedPane,
}

fn split_space(
    space_to_split: &PaneGeom,
    layout: &TiledPaneLayout,
    total_space_to_split: &PaneGeom,
    ignore_percent_split_sizes: bool,
    next_stack_id: &mut usize,
) -> Result<Vec<(TiledPaneLayout, PaneGeom)>, LayoutError> {
    let sizes: Vec<Option<SplitSize>> = if layout.children_are_stacked {
        let index_of_expanded_pane = layout.children.iter().position(|p| p.is_expanded_in_stack);
        let mut sizes: Vec<Option<SplitSize>> = layout
            .children
            .iter()
            .map(|_part| Some(SplitSize::Fixed(1)))
            .collect();
        if let Some(index_of_expanded_pane) = index_of_expanded_pane {
            *sizes.get_mut(index_of_expanded_pane).unwrap() = None;
        } else if let Some(last_size) = sizes.last_mut() {
            *last_size = None;
        }
        // Every member but the expanded one is a single title row, and the expanded one needs
        // MIN_TERMINAL_HEIGHT rows: n members need n - 1 + MIN_TERMINAL_HEIGHT rows.
        let rows = space_to_split.rows.as_usize();
        if sizes.len() > rows.saturating_sub(MIN_TERMINAL_HEIGHT - 1) {
            return Err(LayoutError::InsufficientStackSpace);
        }
        sizes
    } else if ignore_percent_split_sizes {
        layout
            .children
            .iter()
            .map(|part| match part.split_size {
                Some(SplitSize::Percent(_)) => None,
                split_size => split_size,
            })
            .collect()
    } else {
        layout.children.iter().map(|part| part.split_size).collect()
    };

    let mut split_geom = Vec::new();
    let (
        mut current_position,
        split_dimension_space,
        inherited_dimension,
        total_split_dimension_space,
    ) = match layout.children_split_direction {
        SplitDirection::Vertical => (
            space_to_split.x,
            space_to_split.cols,
            space_to_split.rows,
            total_space_to_split.cols,
        ),
        SplitDirection::Horizontal => (
            space_to_split.y,
            space_to_split.rows,
            space_to_split.cols,
            total_space_to_split.rows,
        ),
    };

    let minimum = match layout.children_split_direction {
        SplitDirection::Horizontal => MIN_TERMINAL_HEIGHT,
        SplitDirection::Vertical => MIN_TERMINAL_WIDTH,
    };
    let min_size_for_panes = sizes.iter().fold(0, |acc, size| match size {
        Some(SplitSize::Percent(_)) | None => acc + minimum,
        Some(SplitSize::Fixed(fixed)) => acc + fixed,
    });
    if min_size_for_panes > split_dimension_space.as_usize() {
        return Err(LayoutError::InsufficientSplitSpace);
    }

    let flex_parts = sizes.iter().filter(|s| s.is_none()).count();
    let total_fixed_size = sizes.iter().fold(0, |acc, s| {
        if let Some(SplitSize::Fixed(fixed)) = s {
            acc + fixed
        } else {
            acc
        }
    });
    let stacked = if layout.children_are_stacked {
        let stack_id = *next_stack_id;
        *next_stack_id += 1;
        Some(stack_id)
    } else {
        None
    };

    let mut total_pane_size = 0;
    for (&size, _part) in sizes.iter().zip(&*layout.children) {
        let mut split_dimension = match size {
            Some(SplitSize::Percent(percent)) => Dimension::percent(percent as f64),
            Some(SplitSize::Fixed(size)) => Dimension::fixed(size),
            None => {
                let free_percent = if let Some(p) = split_dimension_space.as_percent() {
                    p - sizes
                        .iter()
                        .map(|&s| match s {
                            Some(SplitSize::Percent(ip)) => ip as f64,
                            _ => 0.0,
                        })
                        .sum::<f64>()
                } else {
                    panic!("Implicit sizing within fixed-size panes is not supported");
                };
                Dimension::percent(free_percent / flex_parts as f64)
            },
        };

        split_dimension.adjust_inner(
            total_split_dimension_space
                .as_usize()
                .saturating_sub(total_fixed_size),
        );
        total_pane_size += split_dimension.as_usize();

        let geom = match layout.children_split_direction {
            SplitDirection::Vertical => PaneGeom {
                x: current_position,
                y: space_to_split.y,
                cols: split_dimension,
                rows: inherited_dimension,
                stacked,
                is_pinned: false,
                logical_position: None,
            },
            SplitDirection::Horizontal => PaneGeom {
                x: space_to_split.x,
                y: current_position,
                cols: inherited_dimension,
                rows: split_dimension,
                stacked,
                is_pinned: false,
                logical_position: None,
            },
        };
        split_geom.push(geom);
        current_position += split_dimension.as_usize();
    }
    adjust_geoms_for_rounding_errors(
        total_pane_size,
        &mut split_geom,
        split_dimension_space,
        layout.children_split_direction,
    );
    let mut pane_positions = Vec::new();
    let mut pane_positions_with_children = Vec::new();
    for (i, part) in layout.children.iter().enumerate() {
        let part_position_and_size = split_geom.get(i).unwrap();
        if !part.children.is_empty() {
            let mut part_positions = split_space(
                part_position_and_size,
                part,
                total_space_to_split,
                ignore_percent_split_sizes,
                next_stack_id,
            )?;
            // add the only first child to pane_positions only adding the others after all the
            // childfree panes have been added so that the returned vec will be sorted breadth-first
            if !part_positions.is_empty() {
                pane_positions.push(part_positions.remove(0));
            }
            pane_positions_with_children.append(&mut part_positions);
        } else {
            let part = part.clone();
            pane_positions.push((part, *part_position_and_size));
        }
    }
    pane_positions.append(&mut pane_positions_with_children);
    if pane_positions.is_empty() {
        let layout = layout.clone();
        pane_positions.push((layout, *space_to_split));
    }
    Ok(pane_positions)
}

fn adjust_geoms_for_rounding_errors(
    total_pane_size: usize,
    split_geoms: &mut [PaneGeom],
    split_dimension_space: Dimension,
    children_split_direction: SplitDirection,
) {
    if total_pane_size < split_dimension_space.as_usize() {
        // add extra space from rounding errors to the last pane

        let increase_by = split_dimension_space
            .as_usize()
            .saturating_sub(total_pane_size);
        let position_of_last_flexible_geom = split_geoms
            .iter()
            .rposition(|s_g| s_g.is_flexible_in_direction(children_split_direction));
        if let Some(mut flexible_geom_and_following_geoms) =
            position_of_last_flexible_geom.map(|p| split_geoms.iter_mut().skip(p))
        {
            if let Some(flexible_geom) = flexible_geom_and_following_geoms.next() {
                match children_split_direction {
                    SplitDirection::Vertical => flexible_geom.cols.increase_inner(increase_by),
                    SplitDirection::Horizontal => flexible_geom.rows.increase_inner(increase_by),
                }
            }
            for following_geom in flexible_geom_and_following_geoms {
                match children_split_direction {
                    SplitDirection::Vertical => {
                        following_geom.x += increase_by;
                    },
                    SplitDirection::Horizontal => {
                        following_geom.y += increase_by;
                    },
                }
            }
        }
    } else if total_pane_size > split_dimension_space.as_usize() {
        // Fixed UI rows can leave more rounding excess than the last pane can absorb.
        let mut remaining = total_pane_size - split_dimension_space.as_usize();
        for index in (0..split_geoms.len()).rev() {
            let (dimension, minimum) = match children_split_direction {
                SplitDirection::Vertical => (&mut split_geoms[index].cols, MIN_TERMINAL_WIDTH),
                SplitDirection::Horizontal => (&mut split_geoms[index].rows, MIN_TERMINAL_HEIGHT),
            };
            if !dimension.is_percent() {
                continue;
            }
            let decrease_by = remaining.min(dimension.as_usize().saturating_sub(minimum));
            dimension.decrease_inner(decrease_by);
            for following_geom in split_geoms.iter_mut().skip(index + 1) {
                match children_split_direction {
                    SplitDirection::Vertical => {
                        following_geom.x = following_geom.x.saturating_sub(decrease_by)
                    },
                    SplitDirection::Horizontal => {
                        following_geom.y = following_geom.y.saturating_sub(decrease_by)
                    },
                }
            }
            remaining -= decrease_by;
            if remaining == 0 {
                break;
            }
        }
    }
}

#[cfg(test)]
#[path = "unit/layout_tests.rs"]
mod layout_tests;
