use yinz_types::data::ConversionError;
use yinz_types::style::Styling;

#[cfg(not(target_family = "wasm"))]
use {yinz_types::data::LayoutInfo, yinz_types::data::LayoutWithError};

use miette::{Diagnostic, LabeledSpan, NamedSource, SourceCode};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use thiserror::Error;

use super::theme::{Themes, UiConfig};
use crate::kdl::action::NotWritable;
use yinz_runtime::home;
use yinz_types::envs::EnvironmentVariables;
use yinz_types::input::context_menu::ContextMenu;
use yinz_types::input::hooks::Hooks;
use yinz_types::input::keybinds::Keybinds;
use yinz_types::input::layout::RunPluginOrAlias;
use yinz_types::input::options::Options;
use {yinz_types::input::plugins::PluginAliases, yinz_types::input::plugins::PluginsConfigError};

pub const DEFAULT_CONFIG_FILE_NAME: &str = "config.kdl";

#[cfg(test)]
#[path = "unit/config_write_test.rs"]
mod config_write_test;

type ConfigResult = Result<Config, ConfigError>;

/// Main configuration.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Config {
    pub keybinds: Keybinds,
    pub options: Options,
    pub themes: Themes,
    pub plugins: PluginAliases,
    pub ui: UiConfig,
    pub env: EnvironmentVariables,
    /// Ordered rather than hashed so that serializing the config — and the
    /// snapshot tests that read it back — produce the same bytes every run.
    pub background_plugins: BTreeSet<RunPluginOrAlias>,
    /// Whether the config opted out of the built-in background plugins, so that
    /// serializing and re-parsing doesn't silently restore them.
    pub background_plugins_cleared: bool,
    /// The plugin bound to each hook point. Defaults to the shipped `agent_exit` screen; see
    /// `input::hooks`.
    pub hooks: Hooks,
    /// The items a right-click menu offers before its hook plugin has had its say; see
    /// `input::context_menu`.
    pub context_menu: ContextMenu,
}

#[derive(Error, Debug, Serialize, Deserialize)]
pub struct KdlError {
    pub error_message: String,
    #[serde(skip)]
    pub src: Option<NamedSource<String>>,
    pub offset: Option<usize>,
    pub len: Option<usize>,
    pub help_message: Option<String>,
}

impl From<KdlError> for yinz_types::data::LayoutDiagnostic {
    fn from(value: KdlError) -> Self {
        Self {
            error_message: value.error_message,
            offset: value.offset,
            len: value.len,
            help_message: value.help_message,
        }
    }
}
impl From<yinz_types::data::LayoutDiagnostic> for KdlError {
    fn from(value: yinz_types::data::LayoutDiagnostic) -> Self {
        Self {
            error_message: value.error_message,
            offset: value.offset,
            len: value.len,
            help_message: value.help_message,
            src: None,
        }
    }
}

impl Clone for KdlError {
    fn clone(&self) -> Self {
        KdlError {
            error_message: self.error_message.clone(),
            src: None, // NamedSource doesn't implement Clone, so we skip it
            offset: self.offset,
            len: self.len,
            help_message: self.help_message.clone(),
        }
    }
}

impl PartialEq for KdlError {
    fn eq(&self, other: &Self) -> bool {
        // Compare everything except src (which doesn't implement PartialEq)
        self.error_message == other.error_message
            && self.offset == other.offset
            && self.len == other.len
            && self.help_message == other.help_message
    }
}

impl Eq for KdlError {}

impl KdlError {
    pub fn add_src(mut self, src_name: String, src_input: String) -> Self {
        self.src = Some(NamedSource::new(src_name, src_input));
        self
    }
}

impl std::fmt::Display for KdlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        write!(f, "Failed to parse Yinz configuration")
    }
}
use std::fmt::Display;

impl Diagnostic for KdlError {
    fn source_code(&self) -> Option<&dyn SourceCode> {
        match self.src.as_ref() {
            Some(src) => Some(src),
            None => None,
        }
    }
    fn help<'a>(&'a self) -> Option<Box<dyn Display + 'a>> {
        match &self.help_message {
            Some(help_message) => Some(Box::new(help_message)),
            None => Some(Box::new(
                "For more information, please see the commented default configuration: `yinz setup --dump-config`".to_string(),
            ))
        }
    }
    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        if let (Some(offset), Some(len)) = (self.offset, self.len) {
            let label = LabeledSpan::new(Some(self.error_message.clone()), offset, len);
            Some(Box::new(std::iter::once(label)))
        } else {
            None
        }
    }
}

/// Where in the configuration an action with no KDL spelling was found, and why it has none.
#[derive(Error, Debug)]
pub enum NotWritableIn {
    #[error("keybinding `{key}` in {modes} mode, action {index}: {source}")]
    Keybind {
        modes: String,
        key: String,
        index: usize,
        #[source]
        source: NotWritable,
    },
    #[error("context menu item `{label}`, action {index}: {source}")]
    ContextMenu {
        label: String,
        index: usize,
        #[source]
        source: NotWritable,
    },
}

/// Why `Config::write_config_to_disk` did not write, or refused what it had written.
#[derive(Debug)]
pub struct ConfigWriteError {
    /// The file that could not be written, or `None` when the text itself was refused before
    /// the file was touched.
    pub path: Option<PathBuf>,
    pub reason: String,
}

impl std::fmt::Display for ConfigWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.reason)
    }
}

impl std::error::Error for ConfigWriteError {}

#[derive(Error, Debug, Diagnostic)]
pub enum ConfigError {
    // Raw parser errors retain their kind until the caller can attach source context.
    #[error("Deserialization error: {0}")]
    KdlDeserializationError(#[from] kdl::KdlError),
    #[error("KdlDeserialization error: {0}")]
    KdlError(KdlError),
    #[error("Config error: {0}")]
    Std(#[from] Box<dyn std::error::Error>),
    // Io error with path context
    #[error("IoError: {0}, File: {1}")]
    IoPath(io::Error, PathBuf),
    // Internal Deserialization Error
    #[error("FromUtf8Error: {0}")]
    FromUtf8(#[from] std::string::FromUtf8Error),
    // Plugins have a semantic error, usually trying to parse two of the same tag
    #[error("PluginsError: {0}")]
    PluginsError(#[from] PluginsConfigError),
    #[error("{0}")]
    ConversionError(#[from] ConversionError),
    #[error("remote layouts are not supported ('{0}'): download the file and pass its local path")]
    RemoteLayoutUnsupported(String),
    #[error("failed to block on async task")]
    Async(#[from] std::io::Error),
    #[error("unsupported configuration format: {0}; use config.kdl")]
    UnsupportedFormat(String),
    #[error("the configuration cannot be written: {0}")]
    NotWritable(#[from] NotWritableIn),
}

impl ConfigError {
    pub fn new_kdl_error(error_message: String, offset: usize, len: usize) -> Self {
        ConfigError::KdlError(KdlError {
            error_message,
            src: None,
            offset: Some(offset),
            len: Some(len),
            help_message: None,
        })
    }
    pub fn new_layout_kdl_error(error_message: String, offset: usize, len: usize) -> Self {
        ConfigError::KdlError(KdlError {
            error_message,
            src: None,
            offset: Some(offset),
            len: Some(len),
            help_message: Some(
                "For more information, please see the example layouts: https://github.com/zacharyhamm/yinz/tree/main/example/layouts".to_string(),
            ),
        })
    }
}

/// Merge `fragment`'s top-level nodes into `managed`: a node replaces the one of the same
/// name, unless both have children, in which case the children are merged one level deep. A
/// child replaces the one with the same name - `agent_exit ""` replaces `agent_exit
/// "file:a.ts"` - except under `keybinds`, where the positional arguments are part of a
/// block's identity: `shared_except "locked"` and `shared_except "normal" "locked"` are
/// different blocks, and one `keybinds` holds several of each, while `normal
/// clear-defaults=true` replaces `normal`. The loaders read children in document
/// order, so a replaced child moves to the end: the newest write always wins. A node declaring
/// `clear-defaults=true` is complete by its own account and replaces its namesake whole, so
/// stale blocks from an earlier fragment do not survive under it. See
/// `Config::write_managed_fragment`.
pub fn merge_kdl_fragment(managed: &mut kdl::KdlDocument, fragment: &kdl::KdlDocument) {
    if let Some(block) = managed.get_mut(crate::editor::EDITS_BLOCK) {
        if let Ok(patches) = crate::editor::patches_from_kdl(block) {
            if let Ok(normalized) = crate::editor::patches_to_kdl(&patches) {
                *block = normalized;
            }
        }
    }
    fn clears_defaults(node: &kdl::KdlNode) -> bool {
        // Lenient on purpose: a non-bool value is rejected when the merged file is parsed.
        node.get("clear-defaults")
            .and_then(|entry| entry.value().as_bool())
            .unwrap_or(false)
    }
    // `KdlEntry`'s own equality includes its whitespace and spelling, so compare the values.
    fn same_arguments(a: &kdl::KdlNode, b: &kdl::KdlNode) -> bool {
        fn arguments(node: &kdl::KdlNode) -> impl Iterator<Item = &kdl::KdlValue> {
            node.entries()
                .iter()
                .filter(|entry| entry.name().is_none())
                .map(kdl::KdlEntry::value)
        }
        arguments(a).eq(arguments(b))
    }
    for node in fragment.nodes() {
        let name = node.name().value();
        if name == crate::editor::EDITS_BLOCK {
            for reset in node
                .children()
                .into_iter()
                .flat_map(|d| d.nodes())
                .filter(|n| n.name().value() == "reset")
            {
                let path: Vec<_> = reset
                    .entries()
                    .iter()
                    .filter_map(|e| e.value().as_string().map(str::to_owned))
                    .collect();
                crate::editor::reset_managed_path(managed, &path);
            }
            let target = match managed
                .nodes_mut()
                .iter()
                .position(|n| n.name().value() == name)
            {
                Some(index) => &mut managed.nodes_mut()[index],
                None => {
                    managed.nodes_mut().push(kdl::KdlNode::new(name));
                    let index = managed.nodes().len() - 1;
                    &mut managed.nodes_mut()[index]
                },
            };
            for edit in node.children().into_iter().flat_map(|d| d.nodes()) {
                let path: Vec<_> = edit.entries().iter().map(|e| e.value()).collect();
                target.ensure_children().nodes_mut().retain(|old| {
                    let old_path: Vec<_> = old.entries().iter().map(|e| e.value()).collect();
                    !old_path.starts_with(&path)
                });
                let has_parent = target.ensure_children().nodes().iter().any(|old| {
                    let old_path: Vec<_> = old.entries().iter().map(|e| e.value()).collect();
                    path.starts_with(&old_path)
                });
                if edit.name().value() != "reset" || has_parent {
                    target.ensure_children().nodes_mut().push(edit.clone());
                }
            }
            continue;
        }
        // Legacy writers must supersede edits to the same entries without dropping siblings.
        if let Some(block) = managed
            .nodes_mut()
            .iter_mut()
            .find(|n| n.name().value() == crate::editor::EDITS_BLOCK)
        {
            if name == "keybinds" {
                // Invalid fragments remain intact for the normal loader to reject before writing.
                if let Ok(reconciled) = crate::editor::reconcile_legacy_keybinds(block, node) {
                    *block = reconciled;
                }
            } else {
                let replace_root =
                    node.children().is_none() || clears_defaults(node) || name == "context_menu";
                let entries: Vec<_> = node
                    .children()
                    .into_iter()
                    .flat_map(|d| d.nodes())
                    .map(|n| n.name().value())
                    .collect();
                let edits = block.ensure_children().nodes_mut();
                for old in edits.iter_mut() {
                    let root = old.get(0).and_then(|entry| entry.value().as_string());
                    if root == Some(name)
                        && old.entries().len() == 1
                        && !replace_root
                        && old.name().value() == "set"
                    {
                        let mut update = kdl::KdlDocument::new();
                        update.nodes_mut().push(node.clone());
                        merge_kdl_fragment(old.ensure_children(), &update);
                    }
                }
                edits.retain(|old| {
                    if old.get(0).and_then(|e| e.value().as_string()) != Some(name) {
                        return true;
                    }
                    if replace_root {
                        return false;
                    }
                    old.get(1)
                        .and_then(|e| e.value().as_string())
                        .is_none_or(|child| !entries.contains(&child))
                });
            }
        }
        let arguments_are_identity = name == "keybinds";
        let existing = managed
            .nodes_mut()
            .iter_mut()
            .find(|candidate| candidate.name().value() == name);
        match existing {
            Some(target)
                if node.children().is_some()
                    && target.children().is_some()
                    && !clears_defaults(node)
                    && name != "context_menu" =>
            {
                let target_children = target.ensure_children().nodes_mut();
                for child in node.children().map(|c| c.nodes()).unwrap_or_default() {
                    target_children.retain(|candidate| {
                        candidate.name().value() != child.name().value()
                            || (arguments_are_identity && !same_arguments(candidate, child))
                    });
                    target_children.push(child.clone());
                }
            },
            Some(target) => *target = node.clone(),
            None => managed.nodes_mut().push(node.clone()),
        }
    }
    // Parsed final nodes may omit a terminator; appended entries must still be valid KDL.
    managed.fmt();
}

impl Config {
    pub fn load(
        config_path: Option<&Path>,
        config_dir: Option<&Path>,
        clean: bool,
    ) -> ConfigResult {
        if clean {
            return Config::from_default_assets();
        }

        // The config *directory*, whichever file is read: the drop-ins hang off it, so a
        // one-off `--config /somewhere/else.kdl` still gets the user's `config.d`, and never
        // `/somewhere/config.d`.
        let config_dir = config_dir
            .map(Path::to_path_buf)
            .or_else(home::find_default_config_dir);
        let overlay_dir = home::get_config_overlay_dir(config_dir.clone());

        if let Some(path) = config_path {
            let default_config = Config::from_default_assets()?;
            return Config::from_path(&path.to_path_buf(), Some(default_config)).and_then(
                |config| {
                    config
                        .with_overlay_dir(overlay_dir.as_deref())
                        .resolve_plugin_repositories(config_path, config_dir.as_deref())
                },
            );
        }

        let config = if let Some(ref config) = config_dir {
            let path = config.join(DEFAULT_CONFIG_FILE_NAME);
            if path.exists() {
                let default_config = Config::from_default_assets()?;
                Config::from_path(&path, Some(default_config))?
            } else {
                Config::from_default_assets()?
            }
        } else {
            Config::from_default_assets()?
        };
        config
            .with_overlay_dir(overlay_dir.as_deref())
            .resolve_plugin_repositories(config_path, config_dir.as_deref())
    }
}

#[cfg(test)]
use yinz_runtime::consts::CONFIG_OVERLAY_DIR_NAME;
/// The drop-in yinz writes itself, for anything it is asked to persist: `yinz action
/// reconfigure --write`, `yinz action set-hook --write`, a plugin's `reconfigure(kdl, true)`.
/// A person's own config file is never rewritten for those.
pub const MANAGED_CONFIG_FILE_NAME: &str = "yinz-managed.kdl";

impl Config {
    pub fn resolve_plugin_repositories(
        mut self,
        config_path: Option<&Path>,
        config_dir: Option<&Path>,
    ) -> ConfigResult {
        let base = config_dir
            .map(Path::to_path_buf)
            .or_else(home::find_default_config_dir)
            .or_else(|| config_path.and_then(Path::parent).map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));
        let base = if base.is_absolute() {
            base
        } else {
            std::env::current_dir()
                .map_err(|e| ConfigError::IoPath(e, base.to_path_buf()))?
                .join(base)
        };
        for plugin in self.plugins.aliases.values_mut() {
            if let yinz_types::input::layout::RunPluginLocation::Git(source) = &mut plugin.location
            {
                yinz_runtime::plugin_location::resolve_repository_path(source, &base)?;
            }
        }
        Ok(self)
    }

    /// Management commands must surface malformed drop-ins too.
    pub fn validate_plugin_overlays(
        config_path: Option<&Path>,
        config_dir: Option<&Path>,
    ) -> Result<(), ConfigError> {
        let config_dir = config_dir
            .map(Path::to_path_buf)
            .or_else(home::find_default_config_dir);
        let base_path = config_path.map(Path::to_path_buf).or_else(|| {
            config_dir
                .as_ref()
                .map(|dir| dir.join(DEFAULT_CONFIG_FILE_NAME))
                .filter(|path| path.exists())
        });
        let mut config = Self::from_default_assets()?;
        if let Some(path) = base_path {
            config = Self::from_path(&path, Some(config))?;
        }
        let directory = home::get_config_overlay_dir(config_dir.clone());
        if let Some(directory) = directory.filter(|path| path.exists()) {
            let mut paths = Vec::new();
            for entry in std::fs::read_dir(&directory)
                .map_err(|e| ConfigError::IoPath(e, directory.clone()))?
            {
                let path = entry
                    .map_err(|e| ConfigError::IoPath(e, directory.clone()))?
                    .path();
                if path.is_file()
                    && path.extension().and_then(|value| value.to_str()) == Some("kdl")
                {
                    paths.push(path);
                }
            }
            paths.sort();
            for path in paths {
                let text = std::fs::read_to_string(&path)
                    .map_err(|e| ConfigError::IoPath(e, path.clone()))?;
                if crate::editor::edits_themes(&text) {
                    apply_themes(&mut config, None, config_dir.clone())?;
                }
                config = Self::from_path(&path, Some(config))?;
            }
        }
        Ok(())
    }

    /// Layer every `*.kdl` in `overlay_dir` over `self`, in file-name order, the way a config
    /// file is layered over the defaults. A drop-in that does not parse is logged and skipped
    /// rather than failing the whole config: a bad managed write must not brick the session.
    /// `None`, or a folder that is not there, is no overlay.
    pub fn with_overlay_dir(self, overlay_dir: Option<&Path>) -> Config {
        let Some(overlay_dir) = overlay_dir else {
            return self;
        };
        let Ok(entries) = std::fs::read_dir(overlay_dir) else {
            return self;
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("kdl"))
            .filter(|path| path.is_file())
            .collect();
        paths.sort();
        paths.into_iter().fold(self, |mut config, path| {
            let text = match std::fs::read_to_string(&path) {
                Ok(text) => text,
                Err(e) => {
                    log::error!("Skipping config drop-in {}: {}", path.display(), e);
                    return config;
                },
            };
            // Leaf edits to a directory theme need that definition before overlay parsing.
            if crate::editor::edits_themes(&text) {
                if let Err(error) = apply_themes(
                    &mut config,
                    None,
                    overlay_dir.parent().map(Path::to_path_buf),
                ) {
                    log::error!("Cannot load themes for {}: {error}", path.display());
                    return config;
                }
            }
            match Config::from_kdl(&text, Some(config.clone())) {
                Ok(layered) => layered,
                Err(e) => {
                    log::error!("Skipping config drop-in {}: {}", path.display(), e);
                    config
                },
            }
        })
    }

    /// Persist a KDL `fragment` into `<overlay_dir>/yinz-managed.kdl`, and answer the path.
    ///
    /// The fragment is merged into what the file already holds by node: a top-level node
    /// replaces the one of the same name - `mouse_mode false` sets that one option - except
    /// that where both have children the children are merged by name, one level deep, so
    /// `hooks { agent_exit "" }` unbinds one point and leaves the others. A `keybinds` mode
    /// block is a child, so it is replaced whole (a `shared_except` block by its name and
    /// arguments); a `keybinds clear-defaults=true` block is a complete keymap and replaces
    /// the whole block. See `merge_kdl_fragment`. The merged file is parsed over the
    /// defaults before anything is written, and the write is atomic, so the file on disk is
    /// always one the loader accepts.
    pub fn write_managed_fragment(overlay_dir: &Path, fragment: &str) -> Result<PathBuf, String> {
        Self::write_managed_fragment_impl(overlay_dir, fragment, true, None, || Ok(()))
    }

    /// Try to write a managed fragment without waiting for another writer's file lock.
    /// Check `guard` under the lock just before replacement; rejection leaves the file unchanged.
    pub fn try_write_managed_fragment_guarded(
        overlay_dir: &Path,
        fragment: &str,
        guard: impl FnOnce() -> Result<(), String>,
    ) -> Result<PathBuf, String> {
        Self::write_managed_fragment_impl(overlay_dir, fragment, false, None, guard)
    }

    pub fn try_write_editor_fragment(
        overlay_dir: &Path,
        fragment: &str,
        base: Config,
        guard: impl FnOnce() -> Result<(), String>,
    ) -> Result<PathBuf, String> {
        Self::write_managed_fragment_impl(overlay_dir, fragment, false, Some(base), guard)
    }

    fn write_managed_fragment_impl(
        overlay_dir: &Path,
        fragment: &str,
        wait_for_lock: bool,
        base: Option<Config>,
        guard: impl FnOnce() -> Result<(), String>,
    ) -> Result<PathBuf, String> {
        use kdl::KdlDocument;
        use std::io::Write;
        let path = overlay_dir.join(MANAGED_CONFIG_FILE_NAME);
        let fragment: KdlDocument = fragment
            .parse()
            .map_err(|e: kdl::KdlError| format!("the config fragment is not valid KDL: {e}"))?;
        std::fs::create_dir_all(overlay_dir)
            .map_err(|e| format!("failed to create {}: {e}", overlay_dir.display()))?;
        // Serialize the read/merge/write across sessions. Keep the lock inode after use.
        let lock = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(overlay_dir.join(format!(".{MANAGED_CONFIG_FILE_NAME}.lock")))
            .map_err(|e| format!("failed to open lock for {}: {e}", path.display()))?;
        if wait_for_lock {
            lock.lock()
                .map_err(|e| format!("failed to lock {}: {e}", path.display()))?;
        } else {
            lock.try_lock().map_err(|e| match e {
                std::fs::TryLockError::WouldBlock => {
                    "Configuration is busy; try saving again".to_owned()
                },
                std::fs::TryLockError::Error(e) => {
                    format!("failed to lock {}: {e}", path.display())
                },
            })?;
        }
        let mut managed: KdlDocument = match std::fs::read_to_string(&path) {
            Ok(text) => text.parse().map_err(|e: kdl::KdlError| {
                format!(
                    "{} is not valid KDL; fix or remove it before yinz can write to it: {e}",
                    path.display()
                )
            })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => KdlDocument::new(),
            Err(e) => return Err(format!("failed to read {}: {e}", path.display())),
        };
        merge_kdl_fragment(&mut managed, &fragment);
        let text = managed.to_string();
        let defaults = match base {
            Some(base) => base,
            None if managed.get(crate::editor::EDITS_BLOCK).is_some() => {
                crate::editor::Disk::read(None, overlay_dir.parent())?
                    .preview(&[], &[])?
                    .2
            },
            None => Config::from_default_assets().map_err(|e| e.to_string())?,
        };
        Config::from_kdl(&text, Some(defaults))
            .map_err(|e| format!("the merged {} would not load: {e}", path.display()))?;
        let mut tmp = tempfile::NamedTempFile::new_in(overlay_dir)
            .map_err(|e| format!("failed to stage {}: {e}", path.display()))?;
        tmp.write_all(text.as_bytes())
            .and_then(|_| tmp.as_file().sync_all())
            .map_err(|e| format!("failed to write {}: {e}", path.display()))?;
        guard()?;
        tmp.persist(&path)
            .map_err(|e| format!("failed to replace {}: {e}", path.display()))?;
        Ok(path)
    }

    pub fn terminal_theme_config(
        &self,
        name: Option<&String>,
    ) -> Option<yinz_types::style::TerminalTheme> {
        self.themes
            .get_theme(name.map(String::as_str).unwrap_or("default"))
            .and_then(|theme| theme.terminal)
    }

    pub fn theme_config(&self, theme_name: Option<&String>) -> Option<Styling> {
        match &theme_name {
            Some(theme_name) => self.themes.get_theme(theme_name).map(|theme| theme.palette),
            None => self.themes.get_theme("default").map(|theme| theme.palette),
        }
    }
    /// Gets default configuration from assets
    pub fn from_default_assets() -> ConfigResult {
        let cfg = String::from_utf8(crate::assets::DEFAULT_CONFIG.to_vec())?;
        match Self::from_kdl(&cfg, None) {
            Ok(config) => Ok(config),
            Err(ConfigError::KdlError(kdl_error)) => Err(ConfigError::KdlError(
                kdl_error.add_src("Default built-in-configuration".into(), cfg),
            )),
            Err(e) => Err(e),
        }
    }
    pub fn from_path(path: &PathBuf, default_config: Option<Config>) -> ConfigResult {
        if path.extension().and_then(|extension| extension.to_str()) == Some("lua") {
            return Err(ConfigError::UnsupportedFormat(
                "Lua configuration is no longer supported".into(),
            ));
        }
        match File::open(path) {
            Ok(mut file) => {
                let mut kdl_config = String::new();
                file.read_to_string(&mut kdl_config)
                    .map_err(|e| ConfigError::IoPath(e, path.to_path_buf()))?;
                match Config::from_kdl(&kdl_config, default_config) {
                    Ok(config) => Ok(config),
                    Err(ConfigError::KdlDeserializationError(kdl_error)) => {
                        let error_message = match kdl_error.kind {
                            kdl::KdlErrorKind::Context("valid node terminator") => {
                                format!("Failed to deserialize KDL node. \nPossible reasons:\n{}\n{}\n{}\n{}",
                                "- Missing `;` after a node name, eg. { node; another_node; }",
                                "- Missing quotations (\") around an argument node eg. { first_node \"argument_node\"; }",
                                "- Missing an equal sign (=) between node arguments on a title line. eg. argument=\"value\"",
                                "- Found an extraneous equal sign (=) between node child arguments and their values. eg. { argument=\"value\" }")
                            },
                            _ => {
                                String::from(kdl_error.help.unwrap_or("Kdl Deserialization Error"))
                            },
                        };
                        let kdl_error = KdlError {
                            error_message,
                            src: Some(NamedSource::new(
                                path.as_path().as_os_str().to_string_lossy(),
                                kdl_config,
                            )),
                            offset: Some(kdl_error.span.offset()),
                            len: Some(kdl_error.span.len()),
                            help_message: None,
                        };
                        Err(ConfigError::KdlError(kdl_error))
                    },
                    Err(ConfigError::KdlError(kdl_error)) => {
                        Err(ConfigError::KdlError(kdl_error.add_src(
                            path.as_path().as_os_str().to_string_lossy().to_string(),
                            kdl_config,
                        )))
                    },
                    Err(e) => Err(e),
                }
            },
            Err(e) => Err(ConfigError::IoPath(e, path.into())),
        }
    }
    pub fn merge(&mut self, other: Config) -> Result<(), ConfigError> {
        self.options = self.options.merge(other.options);
        self.keybinds.merge(other.keybinds.clone());
        self.themes = self.themes.merge(other.themes);
        self.plugins.merge(other.plugins);
        self.ui = self.ui.merge(other.ui);
        self.env = self.env.merge(other.env);
        Ok(())
    }
    pub fn config_file_path(
        config_path: Option<&Path>,
        config_dir: Option<&Path>,
    ) -> Option<PathBuf> {
        config_path.map(Path::to_path_buf).or_else(|| {
            config_dir
                .map(Path::to_path_buf)
                .or_else(|| {
                    home::try_create_home_config_dir();
                    home::find_default_config_dir()
                })
                .map(|config_dir| config_dir.join(DEFAULT_CONFIG_FILE_NAME))
        })
    }
    pub fn default_config_file_path() -> Option<PathBuf> {
        home::find_default_config_dir().map(|config_dir| config_dir.join(DEFAULT_CONFIG_FILE_NAME))
    }
    /// Write `config` - this configuration's own text, from `Config::to_string` - to
    /// `config_file_path`, backing the existing file up first. Refused before the file is
    /// touched when the text does not load back as this configuration's keybindings and
    /// context menu. Stage and validate the replacement before publishing it atomically.
    pub fn write_config_to_disk(
        &self,
        config: String,
        config_file_path: &Path,
    ) -> Result<Config, ConfigWriteError> {
        let at = |path: &Path, reason: String| ConfigWriteError {
            path: Some(path.to_path_buf()),
            reason,
        };
        let parsed_config = Config::from_kdl(&config, None).map_err(|e| ConfigWriteError {
            path: None,
            reason: format!("the configuration text does not parse: {e}"),
        })?;
        // Two parses of the same text agree whatever the serializer left out; only the source
        // can say what it should have said.
        self.keybinds_survive_the_written_form(&parsed_config)
            .map_err(|reason| at(config_file_path, reason))?;
        // Preserve symlink-based dotfile setups. A dangling link is an error, not a file
        // to replace with a new, unrelated configuration.
        let config_file_path = match std::fs::symlink_metadata(config_file_path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                std::fs::canonicalize(config_file_path).map_err(|e| {
                    at(
                        config_file_path,
                        format!("failed to resolve config link: {e}"),
                    )
                })?
            },
            Ok(_) => config_file_path.to_path_buf(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => config_file_path.to_path_buf(),
            Err(e) => {
                return Err(at(
                    config_file_path,
                    format!("failed to inspect config: {e}"),
                ))
            },
        };
        // Atomic replacement needs directory access, but saving must also honor file write access.
        match std::fs::OpenOptions::new()
            .write(true)
            .open(&config_file_path)
        {
            Ok(_) => {},
            Err(e) if e.kind() == io::ErrorKind::NotFound => {},
            Err(e) => {
                return Err(at(
                    &config_file_path,
                    format!("failed to open config for writing: {e}"),
                ))
            },
        }
        let backed_up_file_name =
            Config::backup_current_config(&config_file_path).map_err(|path| ConfigWriteError {
                reason: "failed to back up the current configuration".to_owned(),
                path,
            })?;
        let config = match backed_up_file_name {
            Some(backed_up_file_name) => {
                format!(
                    "{}{}",
                    Config::autogen_config_message(backed_up_file_name),
                    config
                )
            },
            None => config,
        };
        let staged = Self::stage_config(&config_file_path, &config).map_err(|e| {
            at(
                &config_file_path,
                format!("failed to write the configuration: {e}"),
            )
        })?;
        let written_config = std::fs::read_to_string(staged.path()).map_err(|e| {
            at(
                &config_file_path,
                format!("failed to read the written configuration back: {e}"),
            )
        })?;
        let parsed_written_config = Config::from_kdl(&written_config, None).map_err(|e| {
            at(
                &config_file_path,
                format!("the written configuration does not parse: {e}"),
            )
        })?;
        if parsed_written_config == parsed_config {
            staged.persist(&config_file_path).map_err(|e| {
                at(
                    &config_file_path,
                    format!("failed to replace the configuration: {e}"),
                )
            })?;
            Ok(parsed_config)
        } else {
            Err(at(
                &config_file_path,
                "the configuration was corrupted when written to disk".to_owned(),
            ))
        }
    }

    /// Whether every keybinding and context menu item in this configuration is in `parsed`,
    /// the parse of its written form, and nothing else is. `Err` names the first binding that
    /// differs. Layouts are compared after the normalization a `NewTab` binding needs (see the
    /// helpers), which leaves every other action untouched.
    fn keybinds_survive_the_written_form(&self, parsed: &Config) -> Result<(), String> {
        use yinz_types::input::actions::Action;

        use {
            yinz_types::input::layout::FloatingPaneLayout,
            yinz_types::input::layout::TiledPaneLayout,
        };

        // The plugin wire represents an unspecified border as false and carries no pane
        // colours at all, so neither can be compared after an action has crossed it.
        fn normalize_tiled(layout: &mut TiledPaneLayout) {
            if layout.borderless == Some(false) {
                layout.borderless = None;
            }
            layout.default_fg = None;
            layout.default_bg = None;
            for child in &mut layout.children {
                normalize_tiled(child);
            }
        }

        fn normalize_floating(layout: &mut FloatingPaneLayout) {
            layout.default_fg = None;
            layout.default_bg = None;
        }

        // A binding built over the plugin wire can carry a name or cwd and no layout at all;
        // it loads the default layout when it runs, which is exactly what its saved text
        // (`NewTab { name "x" }`) does, except that parsing that text loads the layout eagerly.
        fn implicit_default(action: &Action) -> bool {
            match action {
                Action::NewTab {
                    layout_source: None,
                    tiled_layout: None,
                    floating_layouts,
                    swap_tiled_layouts,
                    swap_floating_layouts,
                    ..
                } => {
                    floating_layouts.is_empty()
                        && swap_tiled_layouts.as_ref().is_none_or(Vec::is_empty)
                        && swap_floating_layouts.as_ref().is_none_or(Vec::is_empty)
                },
                _ => false,
            }
        }

        fn normalized(action: &Action, strip_layout: bool) -> Action {
            let mut action = action.clone();
            if let Action::NewTab {
                layout_source,
                tiled_layout,
                floating_layouts,
                swap_tiled_layouts,
                swap_floating_layouts,
                ..
            } = &mut action
            {
                if layout_source.is_some() || strip_layout {
                    // Only the reference is written; the file it names is the layout's
                    // source of truth, so its current contents are not the binding's state.
                    *tiled_layout = None;
                    floating_layouts.clear();
                    *swap_tiled_layouts = None;
                    *swap_floating_layouts = None;
                    return action;
                }
                if let Some(layout) = tiled_layout {
                    normalize_tiled(layout);
                }
                floating_layouts.iter_mut().for_each(normalize_floating);
                if let Some(layouts) = swap_tiled_layouts {
                    for (constraints, _) in layouts {
                        for layout in constraints.values_mut() {
                            normalize_tiled(layout);
                        }
                    }
                }
                if let Some(layouts) = swap_floating_layouts {
                    for (constraints, _) in layouts {
                        for layouts in constraints.values_mut() {
                            layouts.iter_mut().for_each(normalize_floating);
                        }
                    }
                }
                // Protobuf repeated fields cannot distinguish None from Some([]).
                if swap_tiled_layouts.as_ref().is_some_and(Vec::is_empty) {
                    *swap_tiled_layouts = None;
                }
                if swap_floating_layouts.as_ref().is_some_and(Vec::is_empty) {
                    *swap_floating_layouts = None;
                }
            }
            action
        }

        // Check both directions, including each action's position within its binding.
        for (source, destination) in [(self, parsed), (parsed, self)] {
            for (mode, bindings) in &source.keybinds.0 {
                for (key, actions) in bindings {
                    for (index, action) in actions.iter().enumerate() {
                        let counterpart = destination
                            .keybinds
                            .0
                            .get(mode)
                            .and_then(|bindings| bindings.get(key))
                            .and_then(|actions| actions.get(index));
                        let strip_layout =
                            implicit_default(action) || counterpart.is_some_and(implicit_default);
                        let expected = normalized(action, strip_layout);
                        let found = counterpart.map(|a| normalized(a, strip_layout));
                        if found.as_ref() != Some(&expected) {
                            let hint = if matches!(action, Action::NewTab { .. }) {
                                " If the default layout changed on disk, save or touch the config file so the session re-reads it, then save again."
                            } else {
                                ""
                            };
                            return Err(format!(
                                "keybinding `{}` in {:?} mode, action {}: saved as {:?}, was {:?}.{}",
                                key.to_keybind_string(),
                                mode,
                                index,
                                found,
                                expected,
                                hint
                            ));
                        }
                    }
                }
            }
        }
        if self.context_menu != parsed.context_menu {
            return Err("the context menu is not what its saved form loads".to_owned());
        }
        Ok(())
    }
    // returns true if the config was not previouly written to disk and we successfully wrote it
    pub fn write_config_to_disk_if_it_does_not_exist(
        config: String,
        config_file_path: &Option<PathBuf>,
    ) -> bool {
        let Some(config_file_path) = config_file_path.clone() else {
            log::error!("Could not find file path to write config");
            return false;
        };
        if config_file_path.symlink_metadata().is_ok() {
            return false;
        }
        let written = Self::stage_config(&config_file_path, &config).and_then(|staged| {
            staged
                .persist_noclobber(&config_file_path)
                .map_err(|e| e.error)
        });
        match written {
            Ok(_) => true,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => false,
            Err(e) => {
                log::error!("Failed to write initial config to disk: {}", e);
                false
            },
        }
    }

    fn stage_config(path: &Path, contents: &str) -> io::Result<tempfile::NamedTempFile> {
        use std::io::Write;
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut staged = tempfile::NamedTempFile::new_in(parent)?;
        staged.write_all(contents.as_bytes())?;
        match std::fs::metadata(path) {
            Ok(metadata) => staged.as_file().set_permissions(metadata.permissions())?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {},
            Err(e) => return Err(e),
        }
        staged.as_file().sync_all()?;
        Ok(staged)
    }

    fn backup_current_config(
        config_file_path: &PathBuf,
    ) -> Result<Option<PathBuf>, Option<PathBuf>> {
        let current_config = match std::fs::read_to_string(config_file_path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                log::error!(
                    "Failed to read current config {}: {}",
                    config_file_path.display(),
                    e
                );
                return Err(Some(config_file_path.clone()));
            },
        };
        let mut staged = Self::stage_config(config_file_path, &current_config).map_err(|e| {
            log::error!("Failed to stage config backup: {}", e);
            Some(config_file_path.clone())
        })?;
        let config_file_name = config_file_path
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or(DEFAULT_CONFIG_FILE_NAME);
        for i in 0..100 {
            let new_file_name = if i == 0 {
                format!("{}.bak", config_file_name)
            } else {
                format!("{}.bak.{}", config_file_name, i)
            };
            let mut potential_config_path = config_file_path.to_path_buf();
            potential_config_path.set_file_name(new_file_name);
            match staged.persist_noclobber(&potential_config_path) {
                Ok(_) => return Ok(Some(potential_config_path)),
                Err(e) if e.error.kind() == io::ErrorKind::AlreadyExists => staged = e.file,
                Err(e) => {
                    log::error!(
                        "Failed to back up config file {}: {}",
                        potential_config_path.display(),
                        e
                    );
                    return Err(Some(potential_config_path));
                },
            }
        }
        log::error!(
            "Failed to find a file name to back up the configuration to, ran out of files."
        );
        Err(None)
    }
    fn autogen_config_message(backed_up_file_name: PathBuf) -> String {
        format!("//\n// THIS FILE WAS AUTOGENERATED BY YINZ, THE PREVIOUS FILE AT THIS LOCATION WAS COPIED TO: {}\n//\n\n", backed_up_file_name.display())
    }
}

#[cfg(not(target_family = "wasm"))]
pub async fn watch_config_file_changes<F, Fut>(
    config_file_path: PathBuf,
    config_dir: Option<&Path>,
    on_config_change: F,
) where
    F: Fn(Config) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send,
{
    // in a gist, what we do here is fire the `on_config_change` function whenever there is a
    // change in the config file or the configured theme directory, we do this by:
    // 1. Trying to watch the provided config file for changes
    // 2. If the file is deleted or does not exist, we periodically poll for it (manually, not
    //    through filesystem events)
    // 3. Once it exists, we start watching it for changes again
    //
    // we do this because the alternative is to watch its parent folder and this might cause the
    // classic "too many open files" issue if there are a lot of files there and/or lots of Yinz
    // instances
    use notify::{self, Config as WatcherConfig, Event, PollWatcher, RecursiveMode, Watcher};
    use std::time::Duration;
    use tokio::sync::mpsc;

    /// The theme dir and the `config.d` drop-in dir, as far as they exist: the two folders a
    /// change in which is a change in the config. Watched together with the file.
    fn load_config_and_theme_dir(
        config_file_path: &Path,
        config_dir: Option<&Path>,
    ) -> Option<(Config, Option<PathBuf>)> {
        let config = Config::load(Some(config_file_path), config_dir, false).ok()?;
        let config_dir = config_dir.map(Path::to_path_buf);
        let (_, mut config) = load_layout_and_config(
            None,
            config,
            config_dir.clone(),
            Some(config_file_path),
            None,
            None,
            PathBuf::from("."),
        )
        .ok()?;
        let config_options = apply_themes(&mut config, None, config_dir.clone()).ok()?;
        let theme_dir = config_options.theme_dir.or_else(|| {
            home::get_theme_dir(config_dir.or_else(home::find_default_config_dir))
                .filter(|dir| dir.exists())
        });
        Some((config, theme_dir))
    }

    fn existing_overlay_dir(config_dir: Option<&Path>) -> Option<PathBuf> {
        home::get_config_overlay_dir(
            config_dir
                .map(Path::to_path_buf)
                .or_else(home::find_default_config_dir),
        )
        .filter(|dir| dir.exists())
    }

    fn event_is_for_config_file(event: &Event, config_file_path: &Path) -> bool {
        event.paths.iter().any(|path| path == config_file_path)
    }

    fn event_is_in_theme_dir(event: &Event, theme_dir: Option<&Path>) -> bool {
        theme_dir
            .is_some_and(|theme_dir| event.paths.iter().any(|path| path.starts_with(theme_dir)))
    }

    async fn reload_config_after_change<F, Fut>(
        config_file_path: &Path,
        config_dir: Option<&Path>,
        watched_theme_dir: Option<&Path>,
        watched_overlay_dir: Option<&Path>,
        on_config_change: &F,
    ) -> Option<bool>
    where
        F: Fn(Config) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send,
    {
        tokio::time::sleep(Duration::from_millis(100)).await;

        if !config_file_path.exists() {
            return None;
        }

        let (new_config, new_theme_dir) =
            match load_config_and_theme_dir(config_file_path, config_dir) {
                Some(loaded) => loaded,
                None => {
                    log::error!("Failed to reload config from {:?}", config_file_path);
                    return None;
                },
            };
        on_config_change(new_config).await;
        // A drop-in folder that appeared since the watcher started needs watching too.
        let overlay_dir_changed =
            existing_overlay_dir(config_dir).as_deref() != watched_overlay_dir;
        Some(new_theme_dir.as_deref() != watched_theme_dir || overlay_dir_changed)
    }

    loop {
        if config_file_path.exists() {
            let watched_theme_dir =
                load_config_and_theme_dir(config_file_path.as_path(), config_dir)
                    .and_then(|(_, theme_dir)| theme_dir);
            let watched_overlay_dir = existing_overlay_dir(config_dir);
            let (tx, mut rx) = mpsc::unbounded_channel();

            let mut watcher = match PollWatcher::new(
                move |res: Result<Event, notify::Error>| {
                    let _ = tx.send(res);
                },
                WatcherConfig::default().with_poll_interval(Duration::from_secs(1)),
            ) {
                Ok(watcher) => watcher,
                Err(e) => {
                    log::error!("Failed to create config watcher: {}", e);
                    break;
                },
            };

            if let Err(e) = watcher.watch(&config_file_path, RecursiveMode::NonRecursive) {
                log::error!("Failed to watch config file {:?}: {}", config_file_path, e);
                break;
            }

            if let Some(watched_theme_dir) = &watched_theme_dir {
                if let Err(e) = watcher.watch(watched_theme_dir, RecursiveMode::NonRecursive) {
                    log::error!(
                        "Failed to watch theme dir {:?}, continuing without it: {}",
                        watched_theme_dir,
                        e,
                    );
                }
            }
            if let Some(watched_overlay_dir) = &watched_overlay_dir {
                if let Err(e) = watcher.watch(watched_overlay_dir, RecursiveMode::NonRecursive) {
                    log::error!(
                        "Failed to watch config drop-in dir {:?}, continuing without it: {}",
                        watched_overlay_dir,
                        e,
                    );
                }
            }

            while let Some(event_result) = rx.recv().await {
                let event = match event_result {
                    Ok(event) => event,
                    Err(e) => {
                        log::error!("Config watcher event error: {}", e);
                        break;
                    },
                };

                if event_is_for_config_file(&event, config_file_path.as_path()) {
                    if event.kind.is_remove() {
                        break;
                    }

                    if (event.kind.is_create() || event.kind.is_modify())
                        && reload_config_after_change(
                            config_file_path.as_path(),
                            config_dir,
                            watched_theme_dir.as_deref(),
                            watched_overlay_dir.as_deref(),
                            &on_config_change,
                        )
                        .await
                        .unwrap_or(false)
                    {
                        break;
                    }
                } else if (event_is_in_theme_dir(&event, watched_theme_dir.as_deref())
                    || event_is_in_theme_dir(&event, watched_overlay_dir.as_deref()))
                    && (event.kind.is_remove() || event.kind.is_create() || event.kind.is_modify())
                {
                    let should_restart_watcher = reload_config_after_change(
                        config_file_path.as_path(),
                        config_dir,
                        watched_theme_dir.as_deref(),
                        watched_overlay_dir.as_deref(),
                        &on_config_change,
                    )
                    .await
                    .unwrap_or(true);
                    if should_restart_watcher {
                        break;
                    }
                }
            }
        }

        while !config_file_path.exists() {
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    }
}

#[cfg(not(target_family = "wasm"))]
pub async fn watch_layout_dir_changes<F, Fut>(
    layout_dir: PathBuf,
    default_layout_name: Option<String>,
    on_layout_change: F,
) where
    F: Fn(Vec<LayoutInfo>, Vec<LayoutWithError>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send,
{
    use notify::{self, Config as WatcherConfig, Event, PollWatcher, RecursiveMode, Watcher};
    use std::time::Duration;
    use tokio::sync::mpsc;

    loop {
        if layout_dir.exists() {
            let (tx, mut rx) = mpsc::unbounded_channel();

            let mut watcher = match PollWatcher::new(
                move |res: Result<Event, notify::Error>| {
                    let _ = tx.send(res);
                },
                WatcherConfig::default().with_poll_interval(Duration::from_secs(1)),
            ) {
                Ok(watcher) => watcher,
                Err(_) => break,
            };

            if watcher
                .watch(&layout_dir, RecursiveMode::Recursive)
                .is_err()
            {
                break;
            }

            // Publish after registration too: a reload (or a newly created directory) may
            // already contain layouts without producing any subsequent filesystem events.
            let (layouts, layout_errors) = crate::layout::list_available_layouts(
                Some(layout_dir.clone()),
                &default_layout_name,
            );
            on_layout_change(layouts, layout_errors).await;

            while let Some(event_result) = rx.recv().await {
                match event_result {
                    Ok(event) => {
                        if event.kind.is_remove()
                            || event.kind.is_create()
                            || event.kind.is_modify()
                        {
                            tokio::time::sleep(Duration::from_millis(100)).await;

                            if !layout_dir.exists() {
                                break;
                            }

                            let (layouts, layout_errors) = crate::layout::list_available_layouts(
                                Some(layout_dir.clone()),
                                &default_layout_name,
                            );
                            on_layout_change(layouts, layout_errors).await;
                        }
                    },
                    Err(_) => break,
                }
            }
        }

        while !layout_dir.exists() {
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    }
}
pub fn load_layout_and_config(
    cli_config_options: Option<&Options>,
    config: Config,
    config_dir: Option<PathBuf>,
    config_path: Option<&Path>,
    layout_string: Option<&str>,
    layout_path: Option<&std::path::Path>,
    cwd: PathBuf,
) -> Result<(Option<yinz_types::data::LayoutInfo>, Config), ConfigError> {
    // find the layout folder relative to which we'll look for our layout
    let layout_dir = cli_config_options
        .as_ref()
        .and_then(|cli_options| cli_options.layout_dir.clone())
        .or_else(|| config.options.layout_dir.clone())
        .or_else(|| home::get_layout_dir(config_dir.clone()))
        .or_else(|| home::get_layout_dir(home::find_default_config_dir()))
        // Try to get an absolute path, else let the resolution code figure this out.
        .map(|d| d.canonicalize().unwrap_or(d));
    // Choose the layout source here; the shared layout loader resolves files and builtins.
    let (layout_info, chosen_layout) = if let Some(ref layout_string) = layout_string {
        (
            Some(yinz_types::data::LayoutInfo::Stringified(
                layout_string.to_string(),
            )),
            None,
        )
    } else if let Some(chosen_layout) = layout_path.map(std::path::Path::to_path_buf) {
        let layout_info =
            crate::layout::layout_info_from_cli(&layout_dir, &Some(chosen_layout.clone()), cwd);
        (layout_info, Some(chosen_layout))
    } else {
        let chosen_layout = cli_config_options
            .as_ref()
            .and_then(|cli_options| cli_options.default_layout.clone())
            .or_else(|| config.options.default_layout.clone());
        let layout_info = crate::layout::layout_info_from_config(&layout_dir, &chosen_layout);
        (layout_info, chosen_layout)
    };
    match layout_info {
        Some(yinz_types::data::LayoutInfo::Stringified(ref raw_layout)) => {
            crate::layout::from_stringified_layout(raw_layout, config)
                .map(|(_layout, config)| (layout_info, config))
        },
        _ => {
            crate::layout::from_path_or_default(chosen_layout.as_ref(), layout_dir.clone(), config)
                .map(|(_layout, config)| (layout_info, config))
        },
    }
    .and_then(|(layout_info, config)| {
        config
            .resolve_plugin_repositories(config_path, config_dir.as_deref())
            .map(|config| (layout_info, config))
    })
}
pub fn apply_themes(
    config: &mut Config,
    cli_config_options: Option<Options>,
    config_dir: Option<PathBuf>,
) -> Result<Options, ConfigError> {
    let config_options = match cli_config_options {
        Some(cli_config_options) => config.options.merge(cli_config_options),
        None => config.options.clone(),
    };

    config.themes = crate::assets::get_default_themes().merge(config.themes.clone());

    let user_theme_dir = config_options.theme_dir.clone().or_else(|| {
        home::get_theme_dir(config_dir.clone().or_else(home::find_default_config_dir))
            .filter(|dir| dir.exists())
    });
    if let Some(user_theme_dir) = user_theme_dir {
        config.themes = config.themes.merge(Themes::from_dir(user_theme_dir)?);
    }
    Ok(config_options)
}

#[cfg(test)]
mod config_test {
    use super::*;
    use yinz_types::input::actions::Action;
    use yinz_types::input::hooks::HookPoint;
    use {
        yinz_types::data::InputMode, yinz_types::data::Palette, yinz_types::data::PaletteColor,
        yinz_types::data::StyleDeclaration, yinz_types::data::Styling,
    };

    use std::collections::{BTreeMap, HashMap};
    use std::io::Write;
    use tempfile::tempdir;
    use {
        crate::theme::FrameConfig, crate::theme::Theme, crate::theme::Themes,
        crate::theme::UiConfig,
    };
    use {yinz_types::input::options::Clipboard, yinz_types::input::options::OnForceClose};

    #[test]
    fn explicit_config_must_exist() {
        assert!(Config::load(Some(Path::new("nonexistent.yaml")), None, false).is_err());
    }
    #[test]
    fn clean_skips_an_explicit_missing_config() {
        assert!(Config::load(Some(Path::new("nonexistent.yaml")), None, true).is_ok());
    }
    #[test]
    fn discovery_rejects_an_invalid_config() {
        let tmp = tempdir().unwrap();
        File::create(tmp.path().join(DEFAULT_CONFIG_FILE_NAME))
            .unwrap()
            .write_all(b"keybinds: invalid\n")
            .unwrap();
        assert!(Config::load(None, Some(tmp.path()), false).is_err());
    }
    #[test]
    fn empty_and_missing_config_directories_use_defaults() {
        let tmp = tempdir().unwrap();
        for dir in [tmp.path().to_path_buf(), tmp.path().join("missing")] {
            assert_eq!(
                Config::load(None, Some(&dir), false).unwrap(),
                Config::from_default_assets().unwrap()
            );
        }
    }

    #[test]
    fn can_define_options_in_configfile() {
        let config_contents = r#"
            simplified_ui true
            theme "my cool theme"
            default_mode "locked"
            default_shell "/path/to/my/shell"
            default_cwd "/path"
            default_layout "/path/to/my/layout.kdl"
            layout_dir "/path/to/my/layout-dir"
            builtin_plugins_dir "/path/to/my/builtin-plugins-dir"
            theme_dir "/path/to/my/theme-dir"
            mouse_mode false
            pane_frames false
            mirror_session true
            on_force_close "quit"
            scroll_buffer_size 100000
            copy_command "/path/to/my/copy-command"
            copy_clipboard "primary"
            copy_on_select false
            scrollback_editor "/path/to/my/scrollback-editor"
            session_name "my awesome session"
            attach_to_session true
        "#;
        let config = Config::from_kdl(config_contents, None).unwrap();
        assert_eq!(
            config.options.simplified_ui,
            Some(true),
            "Option set in config"
        );
        assert_eq!(
            config.options.theme,
            Some(String::from("my cool theme")),
            "Option set in config"
        );
        assert_eq!(
            config.options.default_mode,
            Some(InputMode::Locked),
            "Option set in config"
        );
        assert_eq!(
            config.options.default_shell,
            Some(PathBuf::from("/path/to/my/shell")),
            "Option set in config"
        );
        assert_eq!(
            config.options.default_cwd,
            Some(PathBuf::from("/path")),
            "Option set in config"
        );
        assert_eq!(
            config.options.default_layout,
            Some(PathBuf::from("/path/to/my/layout.kdl")),
            "Option set in config"
        );
        assert_eq!(
            config.options.layout_dir,
            Some(PathBuf::from("/path/to/my/layout-dir")),
            "Option set in config"
        );
        assert_eq!(
            config.options.builtin_plugins_dir,
            Some(PathBuf::from("/path/to/my/builtin-plugins-dir")),
            "Option set in config"
        );
        assert_eq!(
            config.options.theme_dir,
            Some(PathBuf::from("/path/to/my/theme-dir")),
            "Option set in config"
        );
        assert_eq!(
            config.options.mouse_mode,
            Some(false),
            "Option set in config"
        );
        assert_eq!(
            config.options.pane_frames,
            Some(false),
            "Option set in config"
        );
        assert_eq!(
            config.options.mirror_session,
            Some(true),
            "Option set in config"
        );
        assert_eq!(
            config.options.on_force_close,
            Some(OnForceClose::Quit),
            "Option set in config"
        );
        assert_eq!(
            config.options.scroll_buffer_size,
            Some(100000),
            "Option set in config"
        );
        assert_eq!(
            config.options.copy_command,
            Some(String::from("/path/to/my/copy-command")),
            "Option set in config"
        );
        assert_eq!(
            config.options.copy_clipboard,
            Some(Clipboard::Primary),
            "Option set in config"
        );
        assert_eq!(
            config.options.copy_on_select,
            Some(false),
            "Option set in config"
        );
        assert_eq!(
            config.options.scrollback_editor,
            Some(PathBuf::from("/path/to/my/scrollback-editor")),
            "Option set in config"
        );
        assert_eq!(
            config.options.session_name,
            Some(Into::into(String::from("my awesome session"))),
            "Option set in config"
        );
        assert_eq!(
            config.options.attach_to_session,
            Some(true),
            "Option set in config"
        );
    }

    #[test]
    fn can_define_themes_in_configfile() {
        let config_contents = r#"
            themes {
                dracula {
                    fg 248 248 242
                    bg 40 42 54
                    red 255 85 85
                    green 80 250 123
                    yellow 241 250 140
                    blue 98 114 164
                    magenta 255 121 198
                    orange 255 184 108
                    cyan 139 233 253
                    black 0 0 0
                    white 255 255 255
                }
            }
        "#;
        let config = Config::from_kdl(config_contents, None).unwrap();
        let mut expected_themes = HashMap::new();
        expected_themes.insert(
            "dracula".into(),
            Theme {
                terminal: None,
                palette: Palette {
                    fg: PaletteColor::Rgb((248, 248, 242)),
                    bg: PaletteColor::Rgb((40, 42, 54)),
                    red: PaletteColor::Rgb((255, 85, 85)),
                    green: PaletteColor::Rgb((80, 250, 123)),
                    yellow: PaletteColor::Rgb((241, 250, 140)),
                    blue: PaletteColor::Rgb((98, 114, 164)),
                    magenta: PaletteColor::Rgb((255, 121, 198)),
                    orange: PaletteColor::Rgb((255, 184, 108)),
                    cyan: PaletteColor::Rgb((139, 233, 253)),
                    black: PaletteColor::Rgb((0, 0, 0)),
                    white: PaletteColor::Rgb((255, 255, 255)),
                    ..Default::default()
                }
                .into(),
                sourced_from_external_file: false,
            },
        );
        let expected_themes = Themes::from_data(expected_themes);
        assert_eq!(config.themes, expected_themes, "Theme defined in config");
    }

    #[test]
    fn can_define_multiple_themes_including_hex_themes_in_configfile() {
        let config_contents = r##"
            themes {
                dracula {
                    fg 248 248 242
                    bg 40 42 54
                    red 255 85 85
                    green 80 250 123
                    yellow 241 250 140
                    blue 98 114 164
                    magenta 255 121 198
                    orange 255 184 108
                    cyan 139 233 253
                    black 0 0 0
                    white 255 255 255
                }
                nord {
                    fg "#D8DEE9"
                    bg "#2E3440"
                    black "#3B4252"
                    red "#BF616A"
                    green "#A3BE8C"
                    yellow "#EBCB8B"
                    blue "#81A1C1"
                    magenta "#B48EAD"
                    cyan "#88C0D0"
                    white "#E5E9F0"
                    orange "#D08770"
                }
            }
        "##;
        let config = Config::from_kdl(config_contents, None).unwrap();
        let mut expected_themes = HashMap::new();
        expected_themes.insert(
            "dracula".into(),
            Theme {
                terminal: None,
                palette: Palette {
                    fg: PaletteColor::Rgb((248, 248, 242)),
                    bg: PaletteColor::Rgb((40, 42, 54)),
                    red: PaletteColor::Rgb((255, 85, 85)),
                    green: PaletteColor::Rgb((80, 250, 123)),
                    yellow: PaletteColor::Rgb((241, 250, 140)),
                    blue: PaletteColor::Rgb((98, 114, 164)),
                    magenta: PaletteColor::Rgb((255, 121, 198)),
                    orange: PaletteColor::Rgb((255, 184, 108)),
                    cyan: PaletteColor::Rgb((139, 233, 253)),
                    black: PaletteColor::Rgb((0, 0, 0)),
                    white: PaletteColor::Rgb((255, 255, 255)),
                    ..Default::default()
                }
                .into(),
                sourced_from_external_file: false,
            },
        );
        expected_themes.insert(
            "nord".into(),
            Theme {
                terminal: None,
                palette: Palette {
                    fg: PaletteColor::Rgb((216, 222, 233)),
                    bg: PaletteColor::Rgb((46, 52, 64)),
                    black: PaletteColor::Rgb((59, 66, 82)),
                    red: PaletteColor::Rgb((191, 97, 106)),
                    green: PaletteColor::Rgb((163, 190, 140)),
                    yellow: PaletteColor::Rgb((235, 203, 139)),
                    blue: PaletteColor::Rgb((129, 161, 193)),
                    magenta: PaletteColor::Rgb((180, 142, 173)),
                    cyan: PaletteColor::Rgb((136, 192, 208)),
                    white: PaletteColor::Rgb((229, 233, 240)),
                    orange: PaletteColor::Rgb((208, 135, 112)),
                    ..Default::default()
                }
                .into(),
                sourced_from_external_file: false,
            },
        );
        let expected_themes = Themes::from_data(expected_themes);
        assert_eq!(config.themes, expected_themes, "Theme defined in config");
    }

    #[test]
    fn can_define_eight_bit_themes() {
        let config_contents = r#"
            themes {
                eight_bit_theme {
                    fg 248
                    bg 40
                    red 255
                    green 80
                    yellow 241
                    blue 98
                    magenta 255
                    orange 255
                    cyan 139
                    black 1
                    white 255
                }
            }
        "#;
        let config = Config::from_kdl(config_contents, None).unwrap();
        let mut expected_themes = HashMap::new();
        expected_themes.insert(
            "eight_bit_theme".into(),
            Theme {
                terminal: None,
                palette: Palette {
                    fg: PaletteColor::EightBit(248),
                    bg: PaletteColor::EightBit(40),
                    red: PaletteColor::EightBit(255),
                    green: PaletteColor::EightBit(80),
                    yellow: PaletteColor::EightBit(241),
                    blue: PaletteColor::EightBit(98),
                    magenta: PaletteColor::EightBit(255),
                    orange: PaletteColor::EightBit(255),
                    cyan: PaletteColor::EightBit(139),
                    black: PaletteColor::EightBit(1),
                    white: PaletteColor::EightBit(255),
                    ..Default::default()
                }
                .into(),
                sourced_from_external_file: false,
            },
        );
        let expected_themes = Themes::from_data(expected_themes);
        assert_eq!(config.themes, expected_themes, "Theme defined in config");
    }

    #[test]
    fn can_define_style_for_theme_with_hex() {
        let config_contents = r##"
            themes {
                named_theme {
                    text_unselected {
                        base "#DCD7BA"
                        emphasis_0 "#DCD7CD"
                        emphasis_1 "#DCD8DD"
                        emphasis_2 "#DCD899"
                        emphasis_3 "#ACD7CD"
                        background   "#1F1F28"
                    }
                    text_selected {
                        base "#16161D"
                        emphasis_0 "#16161D"
                        emphasis_1 "#16161D"
                        emphasis_2 "#16161D"
                        emphasis_3 "#16161D"
                        background   "#9CABCA"
                    }
                    ribbon_unselected {
                        base "#DCD7BA"
                        emphasis_0 "#7FB4CA"
                        emphasis_1 "#A3D4D5"
                        emphasis_2 "#7AA89F"
                        emphasis_3 "#DCD819"
                        background   "#252535"
                    }
                    ribbon_selected {
                        base "#16161D"
                        emphasis_0 "#181820"
                        emphasis_1 "#1A1A22"
                        emphasis_2 "#2A2A37"
                        emphasis_3 "#363646"
                        background   "#76946A"
                    }
                    table_title {
                        base "#DCD7BA"
                        emphasis_0 "#7FB4CA"
                        emphasis_1 "#A3D4D5"
                        emphasis_2 "#7AA89F"
                        emphasis_3 "#DCD819"
                        background   "#252535"
                    }
                    table_cell_unselected {
                        base "#DCD7BA"
                        emphasis_0 "#DCD7CD"
                        emphasis_1 "#DCD8DD"
                        emphasis_2 "#DCD899"
                        emphasis_3 "#ACD7CD"
                        background   "#1F1F28"
                    }
                    table_cell_selected {
                        base "#16161D"
                        emphasis_0 "#181820"
                        emphasis_1 "#1A1A22"
                        emphasis_2 "#2A2A37"
                        emphasis_3 "#363646"
                        background   "#76946A"
                    }
                    list_unselected {
                        base "#DCD7BA"
                        emphasis_0 "#DCD7CD"
                        emphasis_1 "#DCD8DD"
                        emphasis_2 "#DCD899"
                        emphasis_3 "#ACD7CD"
                        background   "#1F1F28"
                    }
                    list_selected {
                        base "#16161D"
                        emphasis_0 "#181820"
                        emphasis_1 "#1A1A22"
                        emphasis_2 "#2A2A37"
                        emphasis_3 "#363646"
                        background   "#76946A"
                    }
                    frame_unselected {
                        base "#DCD8DD"
                        emphasis_0 "#7FB4CA"
                        emphasis_1 "#A3D4D5"
                        emphasis_2 "#7AA89F"
                        emphasis_3 "#DCD819"
                    }
                    frame_selected {
                        base "#76946A"
                        emphasis_0 "#C34043"
                        emphasis_1 "#C8C093"
                        emphasis_2 "#ACD7CD"
                        emphasis_3 "#DCD819"
                    }
                    exit_code_success {
                        base "#76946A"
                        emphasis_0 "#76946A"
                        emphasis_1 "#76946A"
                        emphasis_2 "#76946A"
                        emphasis_3 "#76946A"
                    }
                    exit_code_error {
                        base "#C34043"
                        emphasis_0 "#C34043"
                        emphasis_1 "#C34043"
                        emphasis_2 "#C34043"
                        emphasis_3 "#C34043"
                    }
                }
            }
            "##;

        let config = Config::from_kdl(config_contents, None).unwrap();
        let mut expected_themes = HashMap::new();
        expected_themes.insert(
            "named_theme".into(),
            Theme {
                terminal: None,
                sourced_from_external_file: false,
                palette: Styling {
                    pane_surround: None,
                    text_unselected: StyleDeclaration {
                        base: PaletteColor::Rgb((220, 215, 186)),
                        emphasis_0: PaletteColor::Rgb((220, 215, 205)),
                        emphasis_1: PaletteColor::Rgb((220, 216, 221)),
                        emphasis_2: PaletteColor::Rgb((220, 216, 153)),
                        emphasis_3: PaletteColor::Rgb((172, 215, 205)),
                        background: PaletteColor::Rgb((31, 31, 40)),
                    },
                    text_selected: StyleDeclaration {
                        base: PaletteColor::Rgb((22, 22, 29)),
                        emphasis_0: PaletteColor::Rgb((22, 22, 29)),
                        emphasis_1: PaletteColor::Rgb((22, 22, 29)),
                        emphasis_2: PaletteColor::Rgb((22, 22, 29)),
                        emphasis_3: PaletteColor::Rgb((22, 22, 29)),
                        background: PaletteColor::Rgb((156, 171, 202)),
                    },
                    ribbon_unselected: StyleDeclaration {
                        base: PaletteColor::Rgb((220, 215, 186)),
                        emphasis_0: PaletteColor::Rgb((127, 180, 202)),
                        emphasis_1: PaletteColor::Rgb((163, 212, 213)),
                        emphasis_2: PaletteColor::Rgb((122, 168, 159)),
                        emphasis_3: PaletteColor::Rgb((220, 216, 25)),
                        background: PaletteColor::Rgb((37, 37, 53)),
                    },
                    ribbon_selected: StyleDeclaration {
                        base: PaletteColor::Rgb((22, 22, 29)),
                        emphasis_0: PaletteColor::Rgb((24, 24, 32)),
                        emphasis_1: PaletteColor::Rgb((26, 26, 34)),
                        emphasis_2: PaletteColor::Rgb((42, 42, 55)),
                        emphasis_3: PaletteColor::Rgb((54, 54, 70)),
                        background: PaletteColor::Rgb((118, 148, 106)),
                    },
                    table_title: StyleDeclaration {
                        base: PaletteColor::Rgb((220, 215, 186)),
                        emphasis_0: PaletteColor::Rgb((127, 180, 202)),
                        emphasis_1: PaletteColor::Rgb((163, 212, 213)),
                        emphasis_2: PaletteColor::Rgb((122, 168, 159)),
                        emphasis_3: PaletteColor::Rgb((220, 216, 25)),
                        background: PaletteColor::Rgb((37, 37, 53)),
                    },
                    table_cell_unselected: StyleDeclaration {
                        base: PaletteColor::Rgb((220, 215, 186)),
                        emphasis_0: PaletteColor::Rgb((220, 215, 205)),
                        emphasis_1: PaletteColor::Rgb((220, 216, 221)),
                        emphasis_2: PaletteColor::Rgb((220, 216, 153)),
                        emphasis_3: PaletteColor::Rgb((172, 215, 205)),
                        background: PaletteColor::Rgb((31, 31, 40)),
                    },
                    table_cell_selected: StyleDeclaration {
                        base: PaletteColor::Rgb((22, 22, 29)),
                        emphasis_0: PaletteColor::Rgb((24, 24, 32)),
                        emphasis_1: PaletteColor::Rgb((26, 26, 34)),
                        emphasis_2: PaletteColor::Rgb((42, 42, 55)),
                        emphasis_3: PaletteColor::Rgb((54, 54, 70)),
                        background: PaletteColor::Rgb((118, 148, 106)),
                    },
                    list_unselected: StyleDeclaration {
                        base: PaletteColor::Rgb((220, 215, 186)),
                        emphasis_0: PaletteColor::Rgb((220, 215, 205)),
                        emphasis_1: PaletteColor::Rgb((220, 216, 221)),
                        emphasis_2: PaletteColor::Rgb((220, 216, 153)),
                        emphasis_3: PaletteColor::Rgb((172, 215, 205)),
                        background: PaletteColor::Rgb((31, 31, 40)),
                    },
                    list_selected: StyleDeclaration {
                        base: PaletteColor::Rgb((22, 22, 29)),
                        emphasis_0: PaletteColor::Rgb((24, 24, 32)),
                        emphasis_1: PaletteColor::Rgb((26, 26, 34)),
                        emphasis_2: PaletteColor::Rgb((42, 42, 55)),
                        emphasis_3: PaletteColor::Rgb((54, 54, 70)),
                        background: PaletteColor::Rgb((118, 148, 106)),
                    },
                    frame_unselected: Some(StyleDeclaration {
                        base: PaletteColor::Rgb((220, 216, 221)),
                        emphasis_0: PaletteColor::Rgb((127, 180, 202)),
                        emphasis_1: PaletteColor::Rgb((163, 212, 213)),
                        emphasis_2: PaletteColor::Rgb((122, 168, 159)),
                        emphasis_3: PaletteColor::Rgb((220, 216, 25)),
                        ..Default::default()
                    }),
                    frame_selected: StyleDeclaration {
                        base: PaletteColor::Rgb((118, 148, 106)),
                        emphasis_0: PaletteColor::Rgb((195, 64, 67)),
                        emphasis_1: PaletteColor::Rgb((200, 192, 147)),
                        emphasis_2: PaletteColor::Rgb((172, 215, 205)),
                        emphasis_3: PaletteColor::Rgb((220, 216, 25)),
                        ..Default::default()
                    },
                    exit_code_success: StyleDeclaration {
                        base: PaletteColor::Rgb((118, 148, 106)),
                        emphasis_0: PaletteColor::Rgb((118, 148, 106)),
                        emphasis_1: PaletteColor::Rgb((118, 148, 106)),
                        emphasis_2: PaletteColor::Rgb((118, 148, 106)),
                        emphasis_3: PaletteColor::Rgb((118, 148, 106)),
                        ..Default::default()
                    },
                    exit_code_error: StyleDeclaration {
                        base: PaletteColor::Rgb((195, 64, 67)),
                        emphasis_0: PaletteColor::Rgb((195, 64, 67)),
                        emphasis_1: PaletteColor::Rgb((195, 64, 67)),
                        emphasis_2: PaletteColor::Rgb((195, 64, 67)),
                        emphasis_3: PaletteColor::Rgb((195, 64, 67)),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            },
        );
        let expected_themes = Themes::from_data(expected_themes);
        assert_eq!(config.themes, expected_themes, "Theme defined in config")
    }

    #[test]
    fn omitting_required_style_errors() {
        let config_contents = r##"
            themes {
                named_theme {
                    text_unselected {
                        base "#DCD7BA"
                        emphasis_1 "#DCD8DD"
                        emphasis_2 "#DCD899"
                        emphasis_3 "#ACD7CD"
                        background   "#1F1F28"
                    }
                }
            }
            "##;

        let config = Config::from_kdl(config_contents, None);
        assert!(config.is_err());
        if let Err(ConfigError::KdlError(KdlError {
            error_message,
            src: _,
            offset: _,
            len: _,
            help_message: _,
        })) = config
        {
            assert_eq!(error_message, "Missing theme color: emphasis_0")
        }
    }

    #[test]
    fn partial_declaration_of_styles_defaults_omitted() {
        let config_contents = r##"
            themes {
                named_theme {
                    text_unselected {
                        base "#DCD7BA"
                        emphasis_0 "#DCD7CD"
                        emphasis_1 "#DCD8DD"
                        emphasis_2 "#DCD899"
                        emphasis_3 "#ACD7CD"
                        background   "#1F1F28"
                    }
                }
            }
            "##;

        let config = Config::from_kdl(config_contents, None).unwrap();
        let mut expected_themes = HashMap::new();
        expected_themes.insert(
            "named_theme".into(),
            Theme {
                terminal: None,
                sourced_from_external_file: false,
                palette: Styling {
                    pane_surround: None,
                    text_unselected: StyleDeclaration {
                        base: PaletteColor::Rgb((220, 215, 186)),
                        emphasis_0: PaletteColor::Rgb((220, 215, 205)),
                        emphasis_1: PaletteColor::Rgb((220, 216, 221)),
                        emphasis_2: PaletteColor::Rgb((220, 216, 153)),
                        emphasis_3: PaletteColor::Rgb((172, 215, 205)),
                        background: PaletteColor::Rgb((31, 31, 40)),
                    },
                    ..Default::default()
                },
            },
        );
        let expected_themes = Themes::from_data(expected_themes);
        assert_eq!(config.themes, expected_themes, "Theme defined in config")
    }

    #[test]
    fn can_define_plugin_configuration_in_configfile() {
        let config_contents = r#"
            plugins {
                tab-bar location="yinz:tab-bar"
                status-bar location="yinz:status-bar"
                strider location="yinz:strider"
                compact-bar location="yinz:compact-bar"
                session-manager location="yinz:session-manager"
                welcome-screen location="yinz:session-manager" {
                    welcome_screen true
                }
                filepicker location="yinz:strider"
            }
        "#;
        let config = Config::from_kdl(config_contents, None).unwrap();
        let mut expected_plugin_configuration = BTreeMap::new();
        expected_plugin_configuration.insert(
            "tab-bar".to_owned(),
            yinz_runtime::plugin_location::run_plugin_from_url("yinz:tab-bar").unwrap(),
        );
        expected_plugin_configuration.insert(
            "status-bar".to_owned(),
            yinz_runtime::plugin_location::run_plugin_from_url("yinz:status-bar").unwrap(),
        );
        expected_plugin_configuration.insert(
            "strider".to_owned(),
            yinz_runtime::plugin_location::run_plugin_from_url("yinz:strider").unwrap(),
        );
        expected_plugin_configuration.insert(
            "compact-bar".to_owned(),
            yinz_runtime::plugin_location::run_plugin_from_url("yinz:compact-bar").unwrap(),
        );
        expected_plugin_configuration.insert(
            "session-manager".to_owned(),
            yinz_runtime::plugin_location::run_plugin_from_url("yinz:session-manager").unwrap(),
        );
        let mut welcome_screen_configuration = BTreeMap::new();
        welcome_screen_configuration.insert("welcome_screen".to_owned(), "true".to_owned());
        expected_plugin_configuration.insert(
            "welcome-screen".to_owned(),
            yinz_runtime::plugin_location::run_plugin_from_url("yinz:session-manager")
                .unwrap()
                .with_configuration(welcome_screen_configuration),
        );
        expected_plugin_configuration.insert(
            "filepicker".to_owned(),
            yinz_runtime::plugin_location::run_plugin_from_url("yinz:strider").unwrap(),
        );
        assert_eq!(
            config.plugins,
            PluginAliases::from_data(expected_plugin_configuration),
            "Plugins defined in config"
        );
    }

    #[test]
    fn can_define_ui_configuration_in_configfile() {
        let config_contents = r#"
            ui {
                pane_frames {
                    rounded_corners true
                    hide_session_name true
                }
            }
        "#;
        let config = Config::from_kdl(config_contents, None).unwrap();
        let expected_ui_config = UiConfig {
            pane_frames: FrameConfig {
                rounded_corners: true,
                hide_session_name: true,
            },
        };
        assert_eq!(config.ui, expected_ui_config, "Ui config defined in config");
    }

    #[test]
    fn can_define_env_variables_in_config_file() {
        let config_contents = r#"
            env {
                RUST_BACKTRACE 1
                SOME_OTHER_VAR "foo"
            }
        "#;
        let config = Config::from_kdl(config_contents, None).unwrap();
        let mut expected_env_config = HashMap::new();
        expected_env_config.insert("RUST_BACKTRACE".into(), "1".into());
        expected_env_config.insert("SOME_OTHER_VAR".into(), "foo".into());
        assert_eq!(
            config.env,
            EnvironmentVariables::from_data(expected_env_config),
            "Env variables defined in config"
        );
    }

    #[test]
    fn context_menu_items_parse_labels_and_actions() {
        let config = Config::from_kdl(
            r#"
            context_menu {
                "Copy" {
                    Copy
                }
                separator
                "Close pane" shortcut="Ctrl-w" {
                    CloseFocus
                }
            }
            "#,
            None,
        )
        .unwrap();
        let items = config.context_menu.items();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].label, "Copy");
        assert_eq!(items[0].actions, vec![Action::Copy]);
        assert!(items[1].separator);
        assert_eq!(items[2].label, "Close pane");
        assert_eq!(items[2].shortcut.as_deref(), Some("Ctrl-w"));
        assert_eq!(items[2].actions, vec![Action::CloseFocus]);
    }

    /// Unlike `load_plugins`, a `context_menu` block is not additive: a menu somebody wrote out is
    /// the menu they want. Asserted rather than merely documented, because the divergence is the
    /// sort of thing a later change would "fix" without noticing.
    #[test]
    fn a_context_menu_block_replaces_the_defaults() {
        let config =
            Config::from_kdl("context_menu {\n \"Only this\" {\n Copy\n }\n}", None).unwrap();
        assert_eq!(config.context_menu.items().len(), 1);
        assert_eq!(config.context_menu.items()[0].label, "Only this");
    }

    #[test]
    fn an_empty_context_menu_block_means_no_configured_items() {
        let config = Config::from_kdl("context_menu {\n}", None).unwrap();
        assert!(
            config.context_menu.is_empty(),
            "which is a fine thing to write when the hook plugin supplies everything"
        );
    }

    #[test]
    fn a_config_that_says_nothing_about_the_context_menu_keeps_the_defaults() {
        let config = Config::from_kdl("keybinds {\n}", None).unwrap();
        assert!(!config.context_menu.is_empty());
    }

    #[test]
    fn a_context_menu_item_with_no_actions_is_a_config_error() {
        let err = Config::from_kdl("context_menu {\n \"Does nothing\"\n}", None).unwrap_err();
        let message = format!("{err:?}");
        assert!(message.contains("Does nothing"), "{message}");
    }

    #[test]
    fn a_separator_with_children_is_a_config_error() {
        let err =
            Config::from_kdl("context_menu {\n separator {\n CloseFocus\n }\n}", None).unwrap_err();
        let message = format!("{err:?}");
        assert!(message.contains("separator"), "{message}");
    }

    #[test]
    fn a_separator_with_a_property_is_a_config_error() {
        let err =
            Config::from_kdl("context_menu {\n separator shortcut=\"x\"\n}", None).unwrap_err();
        let message = format!("{err:?}");
        assert!(message.contains("separator"), "{message}");
    }

    #[test]
    fn context_menu_survives_serialization() {
        let config = Config::from_kdl(
            "context_menu {\n \"Copy\" shortcut=\"Ctrl-c\" {\n Copy\n }\n separator\n}",
            None,
        )
        .unwrap();
        let round_tripped = Config::from_kdl(&config.to_string(false).unwrap(), None).unwrap();
        assert_eq!(round_tripped.context_menu, config.context_menu);
    }

    #[test]
    fn the_default_hooks_bind_the_shipped_context_menu() {
        let config = Config::from_kdl("keybinds {\n}", None).unwrap();
        assert_eq!(
            config
                .hooks
                .get(HookPoint::ContextMenu)
                .map(|p| p.location_string()),
            Some("yinz:context-menu".to_owned())
        );
    }

    #[test]
    fn hooks_bind_a_point_to_a_plugin() {
        let config =
            Config::from_kdl("hooks {\n agent_exit \"file:/tmp/mine.ts\"\n}", None).unwrap();
        assert_eq!(
            config
                .hooks
                .get(HookPoint::AgentExit)
                .map(|p| p.location_string()),
            Some("file:/tmp/mine.ts".to_owned())
        );
    }

    #[test]
    fn a_config_that_says_nothing_about_hooks_keeps_the_defaults() {
        let config = Config::from_kdl("keybinds {\n}", None).unwrap();
        assert_eq!(
            config
                .hooks
                .get(HookPoint::AgentExit)
                .map(|p| p.location_string()),
            Some("yinz:agent-exit".to_owned())
        );
    }

    #[test]
    fn an_empty_hook_url_unbinds_the_point() {
        let config = Config::from_kdl("hooks {\n agent_exit \"\"\n}", None).unwrap();
        assert!(
            config.hooks.get(HookPoint::AgentExit).is_none(),
            "an empty string is how a default hook is turned off"
        );
    }

    #[test]
    fn an_unknown_hook_point_is_a_config_error() {
        assert!(Config::from_kdl("hooks {\n agent_started \"file:/tmp/x.ts\"\n}", None).is_err());
    }

    #[test]
    fn every_hook_point_binds_from_kdl_and_survives_serialization() {
        // The KDL surface is generic over `HookPoint::ALL`, and this is what keeps it so: a
        // point that can be named can be bound, and comes back bound.
        for point in HookPoint::ALL {
            let kdl = format!("hooks {{\n {point} \"file:/tmp/{point}.ts\"\n}}");
            let config = Config::from_kdl(&kdl, None).unwrap_or_else(|e| panic!("{point}: {e:?}"));
            assert_eq!(
                config.hooks.get(*point).map(|p| p.location_string()),
                Some(format!("file:/tmp/{point}.ts")),
                "{point}"
            );
            let round_tripped = Config::from_kdl(&config.to_string(false).unwrap(), None).unwrap();
            assert_eq!(round_tripped.hooks, config.hooks, "{point}");
        }
    }

    #[test]
    fn hooks_survive_serialization() {
        // Both directions matter: a bound point must come back bound, and an unbound one must
        // stay unbound rather than falling back to the default.
        let bound =
            Config::from_kdl("hooks {\n agent_exit \"file:/tmp/mine.ts\"\n}", None).unwrap();
        let round_tripped = Config::from_kdl(&bound.to_string(false).unwrap(), None).unwrap();
        assert_eq!(round_tripped.hooks, bound.hooks);

        let unbound = Config::from_kdl("hooks {\n agent_exit \"\"\n}", None).unwrap();
        let round_tripped = Config::from_kdl(&unbound.to_string(false).unwrap(), None).unwrap();
        assert!(round_tripped.hooks.get(HookPoint::AgentExit).is_none());

        // A hook is a whole plugin declaration, so its cwd and configuration have to survive
        // too - otherwise the plugin gets a different `load(config)` after a restart.
        let configured = Config::from_kdl(
            r#"
            hooks {
                agent_exit "file:/tmp/mine.ts" {
                    cwd "/tmp/hook-cwd"
                    greeting "hello"
                    loud true
                }
            }
            "#,
            None,
        )
        .unwrap();
        let round_tripped = Config::from_kdl(&configured.to_string(false).unwrap(), None).unwrap();
        assert_eq!(round_tripped.hooks, configured.hooks);
        // And the written form has to be stable, not merely re-readable: `cwd` is parsed into
        // the configuration as well as into the cwd, so a naive writer duplicates it and the
        // copies multiply with every write.
        assert_eq!(
            Config::from_kdl(&round_tripped.to_string(false).unwrap(), None)
                .unwrap()
                .to_string(false)
                .unwrap(),
            round_tripped.to_string(false).unwrap()
        );
    }

    #[test]
    fn load_plugins_adds_to_defaults_unless_cleared() {
        let base = Config::from_kdl("load_plugins {\n \"yinz:link\"\n}", None).unwrap();
        let names = |config: &Config| -> Vec<String> {
            config
                .background_plugins
                .iter()
                .map(|p| p.location_string())
                .collect()
        };

        let merged = Config::from_kdl(
            "load_plugins {\n \"yinz:configuration\"\n}",
            Some(base.clone()),
        )
        .unwrap();
        assert_eq!(names(&merged), vec!["yinz:configuration", "yinz:link"]);

        let cleared = Config::from_kdl(
            "load_plugins clear-defaults=true {\n \"yinz:configuration\"\n}",
            Some(base),
        )
        .unwrap();
        assert_eq!(names(&cleared), vec!["yinz:configuration"]);
    }

    #[test]
    fn session_palette_is_a_shared_default_and_clear_defaults_removes_it() {
        let config = Config::from_default_assets().unwrap();
        let serialized = config.to_string(false).unwrap();
        assert!(serialized.contains("session-palette"));
        let cleared =
            Config::from_kdl("load_plugins clear-defaults=true {}", Some(config)).unwrap();
        assert!(cleared.background_plugins.is_empty());
    }

    #[test]
    fn load_plugins_clear_defaults_survives_serialization() {
        let base = Config::from_kdl("load_plugins {\n \"yinz:link\"\n}", None).unwrap();
        let names = |config: &Config| -> Vec<String> {
            config
                .background_plugins
                .iter()
                .map(|p| p.location_string())
                .collect()
        };

        let cleared = Config::from_kdl(
            "load_plugins clear-defaults=true {\n \"yinz:configuration\"\n}",
            Some(base.clone()),
        )
        .unwrap();
        let round_tripped =
            Config::from_kdl(&cleared.to_string(false).unwrap(), Some(base.clone())).unwrap();
        assert_eq!(names(&round_tripped), vec!["yinz:configuration"]);

        // the additive case must not grow a clear-defaults out of nowhere
        let merged = Config::from_kdl(
            "load_plugins {\n \"yinz:configuration\"\n}",
            Some(base.clone()),
        )
        .unwrap();
        let round_tripped =
            Config::from_kdl(&merged.to_string(false).unwrap(), Some(base)).unwrap();
        assert_eq!(
            names(&round_tripped),
            vec!["yinz:configuration", "yinz:link"]
        );
    }
}

#[cfg(test)]
mod overlay_tests {
    use super::*;
    use kdl::KdlDocument;

    fn doc(text: &str) -> KdlDocument {
        text.parse().unwrap()
    }

    /// A block replaces its namesake, except that blocks with children merge their children
    /// by name - so one option, or one hook point, can be set without touching the rest.
    #[test]
    fn a_fragment_merges_by_block_and_then_by_child_name() {
        let mut managed =
            doc("mouse_mode true\ncopy_command \"x\"\nhooks {\n    agent_exit \"file:a.ts\"\n}\n");
        merge_kdl_fragment(
            &mut managed,
            &doc("mouse_mode false\nhooks {\n    context_menu \"\"\n}\nload_plugins {\n    \"file:x.ts\"\n}\n"),
        );
        let text = managed.to_string();
        assert!(text.contains("mouse_mode false"), "{text}");
        assert!(!text.contains("mouse_mode true"), "{text}");
        assert!(
            text.contains("copy_command \"x\""),
            "an untouched option stays: {text}"
        );
        assert!(text.contains("agent_exit \"file:a.ts\""), "{text}");
        assert!(text.contains("context_menu \"\""), "{text}");
        assert!(
            text.contains("load_plugins"),
            "a new block is appended: {text}"
        );
    }

    fn loaded(managed: &KdlDocument) -> Config {
        let defaults = Config::from_default_assets().unwrap();
        Config::from_kdl(&managed.to_string(), Some(defaults)).unwrap()
    }

    /// A hook point is keyed by its name: writing `agent_exit ""` over `agent_exit "file:a.ts"`
    /// replaces the binding rather than leaving both for the loader to reconcile.
    #[test]
    fn a_child_with_new_arguments_replaces_its_namesake() {
        let mut managed = doc("hooks {\n    agent_exit \"file:a.ts\"\n}\n");
        merge_kdl_fragment(&mut managed, &doc("hooks {\n    agent_exit \"\"\n}\n"));
        let text = managed.to_string();
        assert_eq!(text.matches("agent_exit").count(), 1, "{text}");
        assert!(
            loaded(&managed)
                .hooks
                .get(yinz_types::input::hooks::HookPoint::AgentExit)
                .is_none(),
            "{text}"
        );
    }

    /// Under `keybinds` the positional arguments are the block's identity: `shared_except`
    /// blocks with different mode lists coexist, and only one with the same list replaces it.
    /// A property is not identity: `normal clear-defaults=true` replaces `normal`.
    #[test]
    fn keybinds_blocks_are_keyed_by_name_and_arguments() {
        let mut managed =
            doc("keybinds {\n    normal {\n        bind \"Ctrl y\" { NewPane; }\n    }\n}\n");
        merge_kdl_fragment(
            &mut managed,
            &doc("keybinds {\n    normal clear-defaults=true {\n        bind \"Ctrl u\" { NewPane; }\n    }\n}\n"),
        );
        let text = managed.to_string();
        assert_eq!(text.matches("normal").count(), 1, "{text}");
        assert!(!text.contains("Ctrl y"), "{text}");

        let mut managed = doc(
            "keybinds {\n    shared_except \"locked\" {\n        bind \"Ctrl y\" { NewPane; }\n    }\n}\n",
        );
        merge_kdl_fragment(
            &mut managed,
            &doc("keybinds {\n    shared_except \"normal\" \"locked\" {\n        bind \"Ctrl u\" { NewPane; }\n    }\n}\n"),
        );
        merge_kdl_fragment(
            &mut managed,
            &doc("keybinds {\n    shared_except \"locked\" {\n        bind \"Ctrl i\" { NewPane; }\n    }\n}\n"),
        );
        let text = managed.to_string();
        assert_eq!(text.matches("shared_except").count(), 2, "{text}");
        assert!(!text.contains("Ctrl y"), "{text}");
        assert!(text.contains("Ctrl u") && text.contains("Ctrl i"), "{text}");
    }

    /// The loaders read children in order, so a replaced block moves to the end and the most
    /// recent write wins: restating a mode block after a `clear-defaults=true` one must not
    /// slide it back in front of the clear.
    #[test]
    fn a_replaced_child_moves_to_the_end() {
        let bind_y = "keybinds {\n    normal {\n        bind \"Ctrl y\" { NewPane; }\n    }\n}\n";
        let mut managed = doc(bind_y);
        merge_kdl_fragment(
            &mut managed,
            &doc("keybinds {\n    normal clear-defaults=true {\n        bind \"Ctrl u\" { NewPane; }\n    }\n}\n"),
        );
        merge_kdl_fragment(&mut managed, &doc(bind_y));
        let config = loaded(&managed);
        let normal = config
            .keybinds
            .0
            .get(&yinz_types::data::InputMode::Normal)
            .unwrap();
        let keys: Vec<String> = normal.keys().map(|key| key.to_string()).collect();
        assert!(
            keys.iter().any(|key| key == "Ctrl y"),
            "{keys:?}\n{managed}"
        );
    }

    /// A `clear-defaults=true` block is a complete keymap: nothing from the earlier block
    /// survives under it.
    #[test]
    fn a_clear_defaults_block_replaces_its_namesake_whole() {
        let mut managed = doc(
            "keybinds {\n    shared_except \"locked\" {\n        bind \"Ctrl y\" { NewPane; }\n    }\n}\n",
        );
        merge_kdl_fragment(
            &mut managed,
            &doc("keybinds clear-defaults=true {\n    normal {\n        bind \"Ctrl u\" { NewPane; }\n    }\n}\n"),
        );
        let text = managed.to_string();
        assert!(!text.contains("shared_except"), "{text}");
        assert!(text.contains("clear-defaults=true"), "{text}");
    }

    #[test]
    fn a_managed_fragment_is_written_merged_and_only_when_it_loads() {
        let dir = tempfile::tempdir().unwrap();
        let overlay_dir = dir.path().join(CONFIG_OVERLAY_DIR_NAME);
        let path = Config::write_managed_fragment(&overlay_dir, "mouse_mode false\n").unwrap();
        assert_eq!(path, overlay_dir.join(MANAGED_CONFIG_FILE_NAME));
        Config::write_managed_fragment(&overlay_dir, "hooks {\n    agent_exit \"\"\n}\n").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("mouse_mode false") && text.contains("agent_exit \"\""),
            "{text}"
        );

        let refused =
            Config::write_managed_fragment(&overlay_dir, "hooks {\n    on_coffee \"x\"\n}\n")
                .unwrap_err();
        assert!(refused.contains("would not load"), "{refused}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            text,
            "a refused write changes nothing"
        );
        assert!(Config::write_managed_fragment(&overlay_dir, "hooks {").is_err());
    }

    #[test]
    fn concurrent_managed_fragments_preserve_every_setting() {
        let dir = tempfile::tempdir().unwrap();
        let barrier = std::sync::Barrier::new(8);
        std::thread::scope(|scope| {
            let workers: Vec<_> = (0..8)
                .map(|i| {
                    let dir = dir.path();
                    let barrier = &barrier;
                    scope.spawn(move || {
                        barrier.wait();
                        Config::write_managed_fragment(
                            dir,
                            &format!("env {{ SETTING_{i} \"{i}\"; }}\n"),
                        )
                        .unwrap();
                    })
                })
                .collect();
            for worker in workers {
                worker.join().unwrap();
            }
        });
        let text = std::fs::read_to_string(dir.path().join(MANAGED_CONFIG_FILE_NAME)).unwrap();
        let managed: kdl::KdlDocument = text.parse().unwrap();
        let env = managed.get("env").unwrap().children().unwrap();
        for i in 0..8 {
            assert_eq!(
                env.get(&format!("SETTING_{i}"))
                    .unwrap()
                    .get(0)
                    .unwrap()
                    .value()
                    .as_string(),
                Some(i.to_string().as_str())
            );
        }
    }

    /// Drop-ins layer in file-name order over the config, and a broken one is skipped.
    #[test]
    fn drop_ins_layer_in_name_order_and_a_broken_one_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let overlay_dir = dir.path().join(CONFIG_OVERLAY_DIR_NAME);
        std::fs::create_dir_all(&overlay_dir).unwrap();
        std::fs::write(overlay_dir.join("10-a.kdl"), "mouse_mode false\n").unwrap();
        std::fs::write(
            overlay_dir.join("20-b.kdl"),
            "mouse_mode true\nhooks {\n    agent_exit \"\"\n}\n",
        )
        .unwrap();
        std::fs::write(overlay_dir.join("30-broken.kdl"), "hooks {").unwrap();
        std::fs::write(overlay_dir.join("notes.txt"), "mouse_mode false\n").unwrap();
        let config = Config::from_default_assets()
            .unwrap()
            .with_overlay_dir(Some(&overlay_dir));
        assert_eq!(
            config.options.mouse_mode,
            Some(true),
            "the later drop-in wins"
        );
        assert!(config
            .hooks
            .get(yinz_types::input::hooks::HookPoint::AgentExit)
            .is_none());
        let untouched = Config::from_default_assets().unwrap();
        assert_eq!(
            untouched
                .clone()
                .with_overlay_dir(Some(&dir.path().join("missing"))),
            untouched
        );
    }
}

#[cfg(test)]
mod kdl_discovery_tests {
    use super::*;
    #[test]
    fn discovery_ignores_lua_and_reloads_kdl_and_overlays() {
        let dir = tempfile::tempdir().unwrap();
        let lua = dir.path().join("config.lua");
        std::fs::write(&lua, "error('must never execute')").unwrap();
        let defaults = Config::from_default_assets().unwrap();
        assert_eq!(
            Config::load(None, Some(dir.path()), false).unwrap(),
            defaults
        );
        let kdl = dir.path().join("config.kdl");
        assert_eq!(
            Config::config_file_path(None, Some(dir.path())),
            Some(kdl.clone())
        );
        std::fs::write(&kdl, "mouse_mode false\n").unwrap();
        assert_eq!(
            Config::load(None, Some(dir.path()), false)
                .unwrap()
                .options
                .mouse_mode,
            Some(false)
        );
        let overlays = dir.path().join(CONFIG_OVERLAY_DIR_NAME);
        std::fs::create_dir(&overlays).unwrap();
        std::fs::write(overlays.join("10-override.kdl"), "mouse_mode true\n").unwrap();
        assert_eq!(
            Config::load(None, Some(dir.path()), false)
                .unwrap()
                .options
                .mouse_mode,
            Some(true)
        );
        std::fs::write(overlays.join("10-override.kdl"), "mouse_mode false\n").unwrap();
        assert_eq!(
            Config::load(None, Some(dir.path()), false)
                .unwrap()
                .options
                .mouse_mode,
            Some(false)
        );
        assert!(Config::load(Some(&lua), Some(dir.path()), false)
            .unwrap_err()
            .to_string()
            .contains("Lua configuration is no longer supported"));
        assert_eq!(
            std::fs::read_to_string(&lua).unwrap(),
            "error('must never execute')"
        );
    }
}
