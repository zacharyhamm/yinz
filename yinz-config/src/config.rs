use yinz_types::data::ConversionError;
use yinz_types::style::Styling;

use {yinz_types::data::LayoutInfo, yinz_types::data::LayoutWithError};

use miette::{Diagnostic, LabeledSpan, NamedSource, SourceCode};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::io;
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
use yinz_types::input::web::WebConfig;
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
    /// None uses the builtin group; Some replaces it, including an empty list.
    #[serde(default)]
    pub overlay_tab_plugins: Option<Vec<yinz_types::pane_tabs::PaneTabDescriptor>>,
    /// The plugin bound to each hook point. Defaults to the shipped `agent_exit` screen; see
    /// `input::hooks`.
    pub hooks: Hooks,
    /// The items a right-click menu offers before its hook plugin has had its say; see
    /// `input::context_menu`.
    pub context_menu: ContextMenu,
    /// The mobile web interface; see `input::web`. Read at session start.
    #[serde(default)]
    pub web: WebConfig,
    #[serde(default)]
    pub terminal: yinz_types::input::native_terminal::TerminalConfig,
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
    /// A concise diagnostic for logs and stderr, where source-span rendering is unavailable.
    pub fn diagnostic_message(&self) -> String {
        match self {
            Self::KdlError(error) => match &error.src {
                Some(source) => format!("{}: {}", source.name(), error.error_message),
                None => error.error_message.clone(),
            },
            _ => self.to_string(),
        }
    }

    pub(crate) fn diagnostic_message_for_file(&self, path: &Path) -> String {
        if matches!(self, Self::KdlError(error) if error.src.is_some())
            || matches!(self, Self::IoPath(..))
        {
            self.diagnostic_message()
        } else {
            format!("{}: {}", path.display(), self.diagnostic_message())
        }
    }

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
/// "file:a.scm"` - except under `keybinds`, where the positional arguments are part of a
/// block's identity: `shared_except "locked"` and `shared_except "normal" "locked"` are
/// different blocks, and one `keybinds` holds several of each, while `normal
/// clear-defaults=true` replaces `normal`. The loaders read children in document
/// order, so a replaced child moves to the end: the newest write always wins. A node declaring
/// `clear-defaults=true` is complete by its own account and replaces its namesake whole, so
/// stale blocks from an earlier fragment do not survive under it. See
/// `Config::write_managed_fragment`.
/// Shared blocks and global unbinds split by resets retain their source identity for replacements.
pub fn merge_kdl_fragment(managed: &mut kdl::KdlDocument, fragment: &kdl::KdlDocument) {
    if let Some(block) = managed.get_mut(crate::editor::EDITS_BLOCK)
        && let Ok(patches) = crate::editor::patches_from_kdl(block)
        && let Ok(normalized) = crate::editor::patches_to_kdl(&patches)
    {
        *block = normalized;
    }
    fn clears_defaults(node: &kdl::KdlNode) -> bool {
        // Lenient on purpose: a non-bool value is rejected when the merged file is parsed.
        node.get("clear-defaults")
            .and_then(|entry| entry.value().as_bool())
            .unwrap_or(false)
    }
    // `KdlEntry`'s own equality includes its whitespace and spelling, so compare the values.
    fn same_keybind_identity(a: &kdl::KdlNode, b: &kdl::KdlNode) -> bool {
        let (Ok(a_source), Ok(b_source)) = (
            crate::editor::keybind_block_source(a),
            crate::editor::keybind_block_source(b),
        ) else {
            // Preserve invalid metadata for the loader to reject before writing.
            return false;
        };
        let a = a_source.as_ref().unwrap_or(a);
        let b = b_source.as_ref().unwrap_or(b);
        fn arguments(node: &kdl::KdlNode) -> impl Iterator<Item = &kdl::KdlValue> {
            node.entries()
                .iter()
                .filter(|entry| entry.name().is_none())
                .map(kdl::KdlEntry::value)
        }
        a.name().value() == b.name().value() && arguments(a).eq(arguments(b))
    }
    fn child_identity<'a>(root: &str, child: &'a str) -> &'a str {
        match (root, child) {
            ("terminal", "font_family") => "font",
            ("terminal", "font_ligatures") => "ligatures",
            _ => child,
        }
    }
    // Validate the incoming terminal section before coalescing blocks or aliases.
    // Keep invalid input intact so the final loader rejects it atomically.
    let terminal_nodes: Vec<_> = fragment
        .nodes()
        .iter()
        .filter(|node| node.name().value() == "terminal")
        .collect();
    let invalid_terminal = terminal_nodes.len() > 1
        || terminal_nodes
            .iter()
            .any(|node| crate::kdl::native_terminal::from_kdl(node).is_err());
    if invalid_terminal {
        managed
            .nodes_mut()
            .retain(|node| node.name().value() != "terminal");
        managed
            .nodes_mut()
            .extend(terminal_nodes.into_iter().cloned());
    }
    for node in fragment.nodes() {
        let name = node.name().value();
        if name == "terminal" && invalid_terminal {
            continue;
        }
        if name == crate::editor::EDITS_BLOCK {
            let normalized = crate::editor::patches_from_kdl(node)
                .ok()
                .and_then(|patches| crate::editor::patches_to_kdl(&patches).ok());
            let Some(node) = normalized.as_ref() else {
                // Keep invalid edits intact for validation before writing, including resets.
                managed.nodes_mut().retain(|old| old.name().value() != name);
                managed.nodes_mut().push(node.clone());
                continue;
            };
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
                let replace_root = node.children().is_none()
                    || clears_defaults(node)
                    || matches!(name, "context_menu" | "overlay_tab_plugins");
                let entries: Vec<_> = node
                    .children()
                    .into_iter()
                    .flat_map(|d| d.nodes())
                    .map(|n| child_identity(name, n.name().value()))
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
                        .is_none_or(|child| !entries.contains(&child_identity(name, child)))
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
                    && !matches!(name, "context_menu" | "overlay_tab_plugins") =>
            {
                let target_children = target.ensure_children().nodes_mut();
                for child in node.children().map(|c| c.nodes()).unwrap_or_default() {
                    target_children.retain(|candidate| {
                        if arguments_are_identity {
                            !same_keybind_identity(candidate, child)
                        } else {
                            child_identity(name, candidate.name().value())
                                != child_identity(name, child.name().value())
                        }
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

fn nonblocking_open_options() -> OpenOptions {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = File::options();
        options.custom_flags(libc::O_NONBLOCK);
        options
    }
    #[cfg(not(unix))]
    File::options()
}

pub(crate) fn open_config_input(path: &Path) -> io::Result<File> {
    nonblocking_open_options().read(true).open(path)
}

pub(crate) fn require_regular_file(file: &File) -> io::Result<std::fs::Metadata> {
    let metadata = file.metadata()?;
    if metadata.is_file() {
        Ok(metadata)
    } else {
        Err(io::Error::other("not a regular file"))
    }
}

fn read_regular_config(path: &Path) -> io::Result<String> {
    let file = open_config_input(path)?;
    require_regular_file(&file)?;
    io::read_to_string(file)
}

pub(crate) fn layout_open_can_fall_back(path: &Path, error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::NotFound
        || path.metadata().is_ok_and(|metadata| metadata.is_file())
}

fn managed_recovery(error: String) -> String {
    format!("{error}. Correct the reported setting or restore a readable regular file at the reported path, then retry.")
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConfigRead {
    Ordinary,
    Checked,
    Saved,
}

impl ConfigRead {
    pub(crate) fn reader(self) -> fn(&Path) -> io::Result<String> {
        match self {
            Self::Ordinary => |path| std::fs::read_to_string(path),
            Self::Checked | Self::Saved => read_regular_config,
        }
    }

    pub(crate) fn optional_layout_text(self, path: &Path) -> Result<Option<String>, ConfigError> {
        if self == Self::Ordinary {
            return Ok(std::fs::read_to_string(path).ok());
        }

        let path_error = |error| ConfigError::IoPath(error, path.into());
        let file = match open_config_input(path) {
            Ok(file) => file,
            Err(error) if layout_open_can_fall_back(path, &error) => return Ok(None),
            Err(error) => return Err(path_error(error)),
        };
        require_regular_file(&file).map_err(path_error)?;
        Ok(io::read_to_string(file).ok())
    }

    pub(crate) fn exists(self, path: &Path) -> Result<bool, ConfigError> {
        match self {
            Self::Ordinary => Ok(path.exists()),
            Self::Checked | Self::Saved => path
                .try_exists()
                .map_err(|error| ConfigError::IoPath(error, path.into())),
        }
    }
}

impl Config {
    pub fn load(
        config_path: Option<&Path>,
        config_dir: Option<&Path>,
        clean: bool,
    ) -> ConfigResult {
        Self::load_with(config_path, config_dir, clean, ConfigRead::Ordinary)
    }

    /// Read runtime layers from checked regular files, skipping invalid drop-ins.
    pub fn load_checked(
        config_path: Option<&Path>,
        config_dir: Option<&Path>,
        clean: bool,
    ) -> ConfigResult {
        Self::load_with(config_path, config_dir, clean, ConfigRead::Checked)
    }

    pub(crate) fn load_with(
        config_path: Option<&Path>,
        config_dir: Option<&Path>,
        clean: bool,
        mode: ConfigRead,
    ) -> ConfigResult {
        if clean {
            return Config::from_default_assets();
        }
        Self::load_layers(config_path, config_dir, mode, mode.reader())
    }

    /// Read the saved layers using regular files. I/O errors that could affect the
    /// winning configuration make the snapshot indeterminate.
    pub fn load_saved(config_path: Option<&Path>, config_dir: Option<&Path>) -> ConfigResult {
        let mode = ConfigRead::Saved;
        Self::load_layers(config_path, config_dir, mode, mode.reader())
    }

    fn load_layers(
        config_path: Option<&Path>,
        config_dir: Option<&Path>,
        mode: ConfigRead,
        read_main: impl FnOnce(&Path) -> io::Result<String>,
    ) -> ConfigResult {
        // The config *directory*, whichever file is read: the drop-ins hang off it, so a
        // one-off `--config /somewhere/else.kdl` still gets the user's `config.d`, and never
        // `/somewhere/config.d`.
        let config_dir = config_dir
            .map(Path::to_path_buf)
            .or_else(home::find_default_config_dir);
        let overlay_dir = home::get_config_overlay_dir(config_dir.clone());

        let mut config = Config::from_default_assets()?;
        let default_path;
        let main = match (config_path, config_dir.as_deref()) {
            (Some(path), _) => Some(path),
            (None, Some(dir)) => {
                default_path = dir.join(DEFAULT_CONFIG_FILE_NAME);
                mode.exists(&default_path)?
                    .then_some(default_path.as_path())
            },
            (None, None) => None,
        };
        if let Some(path) = main {
            config = Config::from_path_with(path, Some(config), mode, read_main)?;
        }
        config.load_overlay_dir(overlay_dir.as_deref(), mode)?;
        config.resolve_plugin_repositories(config_path, config_dir.as_deref())
    }
}

#[cfg(test)]
use yinz_runtime::consts::CONFIG_OVERLAY_DIR_NAME;
/// The drop-in yinz writes itself, for anything it is asked to persist: `yinz action
/// reconfigure --write`, `yinz action set-hook --write`, a plugin's `reconfigure(kdl, true)`.
/// A person's own config file is never rewritten for those.
pub const MANAGED_CONFIG_FILE_NAME: &str = "yinz-managed.kdl";

/// What the first start writes to a missing `config.kdl`: comments only, no settings. The
/// shipped defaults stay embedded, so a later release's defaults are not shadowed by a copy
/// frozen on disk. The file still has to exist:
///
/// - Its no-clobber creation is the first-run signal. The server launches the setup wizard
///   only when it created this file, so the wizard runs once per config directory.
/// - The config watcher follows `config.d` and the themes directory only while it exists.
/// - It is where a person writes their own settings.
///
/// It must stay valid KDL without nodes: any node here becomes an override of the defaults.
pub const FIRST_RUN_CONFIG: &str = "\
// Yinz configuration.
//
// Settings in this file override Yinz's built-in defaults, which are not copied here.
// Run `yinz setup --dump-config` to print every option with its default value.
//
// Settings saved from inside Yinz (the setup wizard, the configuration plugin or
// `yinz action reconfigure --write`) go to config.d/yinz-managed.kdl, not to this file.
//
// Files in config.d are applied after this file, so a setting or keybinding saved there
// replaces the same one here. A keymap chosen in the setup wizard replaces all keybinds in
// this file. To use this file's setting instead, remove that entry from
// config.d/yinz-managed.kdl.
";

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
    pub fn with_overlay_dir(mut self, overlay_dir: Option<&Path>) -> Config {
        if let Err(error) = self.load_overlay_dir(overlay_dir, ConfigRead::Ordinary) {
            log::error!(
                "Failed to load config drop-ins: {}",
                error.diagnostic_message()
            );
        }
        self
    }

    fn load_overlay_dir(
        &mut self,
        overlay_dir: Option<&Path>,
        mode: ConfigRead,
    ) -> Result<(), ConfigError> {
        let Some(overlay_dir) = overlay_dir else {
            return Ok(());
        };
        let entries = match std::fs::read_dir(overlay_dir) {
            Ok(entries) => entries,
            Err(error) if mode == ConfigRead::Saved && error.kind() != io::ErrorKind::NotFound => {
                return Err(ConfigError::IoPath(error, overlay_dir.into()));
            },
            Err(_) => return Ok(()),
        };
        let mut paths = Vec::new();
        for entry in entries {
            let path = match entry {
                Ok(entry) => entry.path(),
                Err(error) if mode == ConfigRead::Saved => {
                    return Err(ConfigError::IoPath(error, overlay_dir.into()));
                },
                Err(_) => continue,
            };
            if path.extension().and_then(|ext| ext.to_str()) != Some("kdl") {
                continue;
            }
            match path.metadata() {
                Ok(metadata) if metadata.is_file() => paths.push(path),
                Err(error) if mode == ConfigRead::Saved => {
                    return Err(ConfigError::IoPath(error, path));
                },
                _ => {},
            }
        }
        paths.sort();
        let read = mode.reader();
        for path in paths {
            let text = match read(&path) {
                Ok(text) => text,
                Err(error) if mode == ConfigRead::Saved => {
                    return Err(ConfigError::IoPath(error, path));
                },
                Err(e) => {
                    log::error!("Skipping config drop-in {}: {}", path.display(), e);
                    continue;
                },
            };
            // Leaf edits to a directory theme need that definition before overlay parsing.
            let mut base = self.clone();
            if crate::editor::edits_themes(&text)
                && let Err(error) = apply_themes_with(
                    &mut base,
                    None,
                    overlay_dir.parent().map(Path::to_path_buf),
                    mode,
                )
            {
                if mode == ConfigRead::Saved && matches!(error, ConfigError::IoPath(..)) {
                    return Err(error);
                }
                log::error!(
                    "Cannot load themes for {}: {}",
                    path.display(),
                    error.diagnostic_message()
                );
                continue;
            }
            match Config::from_file_text(&path, text, Some(base), mode) {
                Ok(layered) => *self = layered,
                Err(error @ ConfigError::IoPath(..)) if mode == ConfigRead::Saved => {
                    return Err(error);
                },
                Err(error) => log::error!(
                    "Skipping config drop-in {}",
                    error.diagnostic_message_for_file(&path)
                ),
            }
        }
        Ok(())
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
        let lock_path = overlay_dir.join(format!(".{MANAGED_CONFIG_FILE_NAME}.lock"));
        let lock_error = |error| {
            format!(
                "Cannot use managed configuration lock {}: {error}. Restore a usable regular lock file at that path, then retry.",
                lock_path.display()
            )
        };
        let lock = nonblocking_open_options()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(lock_error)?;
        require_regular_file(&lock).map_err(lock_error)?;
        if wait_for_lock {
            lock.lock().map_err(lock_error)?;
        } else {
            lock.try_lock().map_err(|e| match e {
                std::fs::TryLockError::WouldBlock => {
                    "Configuration is busy; try saving again".to_owned()
                },
                std::fs::TryLockError::Error(e) => lock_error(e),
            })?;
        }
        let mode = ConfigRead::Saved;
        let mut managed: KdlDocument = match mode.reader()(&path) {
            Ok(text) => text.parse().map_err(|e: kdl::KdlError| {
                format!(
                    "{} is not valid KDL; fix or remove it before yinz can write to it: {e}",
                    path.display()
                )
            })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => KdlDocument::new(),
            Err(e) => {
                return Err(managed_recovery(format!(
                    "Cannot read {}: {e}",
                    path.display()
                )));
            },
        };
        merge_kdl_fragment(&mut managed, &fragment);
        let text = managed.to_string();
        let defaults = match base {
            Some(base) => base,
            None if managed.get(crate::editor::EDITS_BLOCK).is_some() => {
                crate::editor::Disk::read_with(None, overlay_dir.parent(), mode)
                    .and_then(|disk| disk.preview_with(&[], &[], mode))
                    .map_err(managed_recovery)?
                    .2
            },
            None => Config::from_default_assets().map_err(|e| e.to_string())?,
        };
        Config::from_kdl_with(&text, Some(defaults), mode).map_err(|error| {
            managed_recovery(format!(
                "The merged configuration would not load. {}",
                error.diagnostic_message_for_file(&path)
            ))
        })?;
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
        let mode = ConfigRead::Ordinary;
        Self::from_path_with(path, default_config, mode, mode.reader())
    }

    fn from_path_with(
        path: &Path,
        default_config: Option<Config>,
        mode: ConfigRead,
        read: impl FnOnce(&Path) -> io::Result<String>,
    ) -> ConfigResult {
        if path.extension().and_then(|extension| extension.to_str()) == Some("lua") {
            return Err(ConfigError::UnsupportedFormat(
                "Lua configuration is no longer supported".into(),
            ));
        }
        let kdl_config = read(path).map_err(|error| ConfigError::IoPath(error, path.into()))?;
        Self::from_file_text(path, kdl_config, default_config, mode)
    }

    pub(crate) fn from_file_text(
        path: &Path,
        kdl_config: String,
        default_config: Option<Config>,
        mode: ConfigRead,
    ) -> ConfigResult {
        match Config::from_kdl_with(&kdl_config, default_config, mode) {
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
                    _ => String::from(kdl_error.help.unwrap_or("Kdl Deserialization Error")),
                };
                let kdl_error = KdlError {
                    error_message,
                    src: Some(NamedSource::new(
                        path.as_os_str().to_string_lossy(),
                        kdl_config,
                    )),
                    offset: Some(kdl_error.span.offset()),
                    len: Some(kdl_error.span.len()),
                    help_message: None,
                };
                Err(ConfigError::KdlError(kdl_error))
            },
            Err(ConfigError::KdlError(kdl_error)) => Err(ConfigError::KdlError(
                kdl_error.add_src(path.as_os_str().to_string_lossy().to_string(), kdl_config),
            )),
            Err(e) => Err(e),
        }
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

pub async fn watch_config_file_changes<F, Fut>(
    config_file_path: PathBuf,
    config_dir: Option<&Path>,
    on_config_change: F,
) where
    F: Fn(Config) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send,
{
    watch_config_file_changes_inner(
        config_file_path,
        config_dir,
        on_config_change,
        || {},
        || {},
        |_| {},
    )
    .await;
}

// Tests can observe registration, main-file loss and reads through the production loop.
async fn watch_config_file_changes_inner<F, Fut>(
    config_file_path: PathBuf,
    config_dir: Option<&Path>,
    on_config_change: F,
    on_ready: impl FnOnce() + Send,
    on_main_lost: impl Fn() + Send,
    on_main_read: impl Fn(&str) + Send,
) where
    F: Fn(Config) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send,
{
    use notify::{self, Config as WatcherConfig, Event, PollWatcher, RecursiveMode, Watcher};
    use std::time::{Duration, SystemTime};
    use tokio::sync::mpsc;
    use tokio::time::Instant;
    use yinz_runtime::errors::prelude::*;

    const SETTLE: Duration = Duration::from_millis(100);

    #[derive(PartialEq, Eq)]
    struct MainConfig {
        text: String,
        modified: Option<SystemTime>,
    }

    // PollWatcher's content comparison opens paths separately from its file-type check.
    // Keep observation and parsing of the main file tied to one checked descriptor.
    fn read_main(path: &Path) -> io::Result<MainConfig> {
        let file = open_config_input(path)?;
        let modified = require_regular_file(&file)?.modified().ok();
        let text = io::read_to_string(file)?;
        Ok(MainConfig { text, modified })
    }

    fn load_config_and_theme_dir(
        config_file_path: &Path,
        config_dir: Option<&Path>,
        main_text: &str,
    ) -> Result<(Config, Option<PathBuf>), ConfigError> {
        let mode = ConfigRead::Checked;
        let config = Config::load_layers(Some(config_file_path), config_dir, mode, |_| {
            Ok(main_text.to_owned())
        })?;
        let config_dir = config_dir.map(Path::to_path_buf);
        let mut config =
            load_default_layout_config(config, config_dir.clone(), Some(config_file_path), mode)?;
        let config_options = apply_themes_with(&mut config, None, config_dir.clone(), mode)?;
        let theme_dir = config_options.theme_dir.or_else(|| {
            home::get_theme_dir(config_dir.or_else(home::find_default_config_dir))
                .filter(|dir| dir.exists())
        });
        Ok((config, theme_dir))
    }

    fn expected_overlay_dir(config_dir: Option<&Path>) -> Option<PathBuf> {
        home::get_config_overlay_dir(
            config_dir
                .map(Path::to_path_buf)
                .or_else(home::find_default_config_dir),
        )
    }

    fn existing_overlay_dir(config_dir: Option<&Path>) -> Option<PathBuf> {
        expected_overlay_dir(config_dir).filter(|dir| dir.is_dir())
    }

    fn event_is_in_theme_dir(event: &Event, theme_dir: Option<&Path>) -> bool {
        theme_dir
            .is_some_and(|theme_dir| event.paths.iter().any(|path| path.starts_with(theme_dir)))
    }

    fn log_main_read_error(path: &Path, error: &io::Error) {
        log::error!("Failed to reload config from {}: {error}", path.display());
    }

    let mut on_ready = Some(on_ready);
    let mut last_main = None;
    let mut dependency_retry_pending = false;
    let mut last_retry_diagnostic = None;
    let mut recover_on_registration = false;
    loop {
        // Wait on the main path alone when it is missing. An unusable path still
        // gets a probe and directory watches so an in-place repair is observed.
        let initial_main = loop {
            match read_main(&config_file_path) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    tokio::time::sleep(Duration::from_secs(3)).await;
                },
                result => break result,
            }
        };
        let watched_theme_dir = initial_main.as_ref().ok().and_then(|main| {
            match load_config_and_theme_dir(&config_file_path, config_dir, &main.text) {
                Ok((_, theme_dir)) => theme_dir,
                Err(error) => {
                    // A theme directory cannot be discovered when a dependency is
                    // unusable. Retry through the same publication path.
                    if matches!(error, ConfigError::IoPath(..)) {
                        dependency_retry_pending = true;
                        recover_on_registration = true;
                    }
                    None
                },
            }
        });
        if last_main.is_none() {
            if let Err(error) = &initial_main {
                log_main_read_error(&config_file_path, error);
            }
            last_main = Some(initial_main);
        }
        let mut watched_overlay_dir = existing_overlay_dir(config_dir);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut watcher = match PollWatcher::new(
            move |res: Result<Event, notify::Error>| {
                let _ = tx.send(res);
            },
            // A newly discovered drop-in can be edited again in the same second.
            // PollWatcher's default mtime comparison rounds to whole seconds.
            WatcherConfig::default()
                .with_poll_interval(Duration::from_secs(1))
                .with_compare_contents(true),
        ) {
            Ok(watcher) => watcher,
            Err(error) => {
                log::error!("Failed to create config watcher: {error}");
                break;
            },
        };

        if let Some(directory) = &watched_theme_dir
            && let Err(error) = watcher.watch(directory, RecursiveMode::NonRecursive)
        {
            log::error!("Failed to watch theme dir {directory:?}, continuing without it: {error}");
        }
        let mut overlay_watch_failed = false;
        if let Some(directory) = &watched_overlay_dir
            && let Err(error) = watcher.watch(directory, RecursiveMode::NonRecursive)
        {
            log::error!("Failed to watch config drop-in dir {directory:?}: {error}");
            watched_overlay_dir = None;
            overlay_watch_failed = true;
        }
        if let Some(on_ready) = on_ready.take() {
            on_ready();
        }

        let mut presence_probe = tokio::time::interval(Duration::from_secs(1));
        presence_probe.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut settle_at = None;
        let main_lost = loop {
            // After a missing main returns, load the full configuration after the
            // directories are registered, so their initial snapshots cannot hide a write.
            let reload_layers = if std::mem::take(&mut recover_on_registration) {
                true
            } else {
                tokio::select! {
                    event = rx.recv() => {
                        match event {
                            None => break false,
                            Some(Err(error)) => {
                                log::error!("Config watcher event error: {error}");
                                if watched_overlay_dir.as_ref().is_some_and(|directory| {
                                    !directory.is_dir()
                                        || error.paths.iter().any(|path| path.starts_with(directory))
                                }) {
                                    overlay_watch_failed = true;
                                    continue;
                                }
                                break false;
                            },
                            Some(Ok(event)) => {
                                if !(event_is_in_theme_dir(&event, watched_theme_dir.as_deref())
                                    || event_is_in_theme_dir(&event, watched_overlay_dir.as_deref()))
                                    || !(event.kind.is_remove() || event.kind.is_create() || event.kind.is_modify())
                                {
                                    continue;
                                }
                                true
                            },
                        }
                    },
                    _ = presence_probe.tick() => {
                        let overlay_dir = existing_overlay_dir(config_dir);
                        let changed = overlay_dir != watched_overlay_dir || overlay_watch_failed;
                        if changed {
                            overlay_watch_failed = false;
                            if let Some(previous) = watched_overlay_dir.take() {
                                watcher.unwatch(&previous).context("unwatch removed config drop-in directory").non_fatal();
                            }
                            if let Some(directory) = overlay_dir {
                                match watcher.watch(&directory, RecursiveMode::NonRecursive) {
                                    Ok(()) => watched_overlay_dir = Some(directory),
                                    Err(error) => {
                                        log::error!("Failed to watch config drop-in dir {directory:?}: {error}");
                                        overlay_watch_failed = true;
                                    },
                                }
                            }
                            // Register before loading so a racing write is read or observed.
                        }
                        (changed && !overlay_watch_failed) || dependency_retry_pending
                    },
                    _ = tokio::time::sleep_until(settle_at.unwrap_or_else(Instant::now)),
                        if settle_at.is_some() => false,
                }
            };
            let current = read_main(&config_file_path);
            if let Ok(main) = &current {
                on_main_read(&main.text);
            }
            let changed = match (last_main.as_ref(), &current) {
                (Some(Ok(previous)), Ok(main)) => previous != main,
                (Some(Err(previous)), Err(error)) => previous.kind() != error.kind(),
                _ => true,
            };
            let main = match last_main.insert(current) {
                Ok(main) => main,
                Err(error) if error.kind() == io::ErrorKind::NotFound => break true,
                Err(error) => {
                    settle_at = None;
                    if changed {
                        log_main_read_error(&config_file_path, error);
                    }
                    continue;
                },
            };
            if changed || reload_layers {
                settle_at = Some(Instant::now() + SETTLE);
                continue;
            }
            if !settle_at.is_some_and(|deadline| Instant::now() >= deadline) {
                continue;
            }
            // The read after the deadline still matches the candidate.
            settle_at = None;

            let (config, theme_dir) = match load_config_and_theme_dir(
                &config_file_path,
                config_dir,
                &main.text,
            ) {
                Ok(loaded) => loaded,
                Err(error) => {
                    let diagnostic = if matches!(error, ConfigError::IoPath(..)) {
                        dependency_retry_pending = true;
                        format!(
                            "Failed to reload config: {}. Restore a readable regular file at the reported path.",
                            error.diagnostic_message()
                        )
                    } else {
                        format!(
                            "Failed to reload config from {}",
                            error.diagnostic_message_for_file(&config_file_path)
                        )
                    };
                    if last_retry_diagnostic.as_ref() != Some(&diagnostic) {
                        log::error!("{diagnostic}");
                        if dependency_retry_pending {
                            last_retry_diagnostic = Some(diagnostic);
                        }
                    }
                    continue;
                },
            };
            dependency_retry_pending = false;
            last_retry_diagnostic = None;
            on_config_change(config).await;
            if theme_dir != watched_theme_dir {
                break false;
            }
        };
        // If the directory watcher had to restart mid-settle, the next
        // registration must still publish the pending snapshot.
        recover_on_registration |= settle_at.is_some();
        if main_lost {
            drop(watcher);
            on_main_lost();
            recover_on_registration = true;
        }
    }
}

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
fn configured_layout_dir(
    cli_config_options: Option<&Options>,
    config: &Config,
    config_dir: Option<PathBuf>,
) -> Option<PathBuf> {
    cli_config_options
        .as_ref()
        .and_then(|cli_options| cli_options.layout_dir.clone())
        .or_else(|| config.options.layout_dir.clone())
        .or_else(|| home::get_layout_dir(config_dir))
        .or_else(|| home::get_layout_dir(home::find_default_config_dir()))
        // Try to get an absolute path, else let the resolution code figure this out.
        .map(|d| d.canonicalize().unwrap_or(d))
}

pub(crate) fn load_default_layout_config(
    config: Config,
    config_dir: Option<PathBuf>,
    config_path: Option<&Path>,
    mode: ConfigRead,
) -> ConfigResult {
    let layout_dir = configured_layout_dir(None, &config, config_dir.clone());
    let layout_path = config.options.default_layout.clone();
    let (_, config) =
        crate::layout::from_path_or_default_with(layout_path.as_ref(), layout_dir, config, mode)?;
    config.resolve_plugin_repositories(config_path, config_dir.as_deref())
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
    load_layout_and_config_with(
        cli_config_options,
        config,
        config_dir,
        config_path,
        layout_string,
        layout_path,
        cwd,
        ConfigRead::Ordinary,
    )
}

/// Load the selected layout and its nested inputs through checked runtime reads.
pub fn load_checked_layout_and_config(
    cli_config_options: Option<&Options>,
    config: Config,
    config_dir: Option<PathBuf>,
    config_path: Option<&Path>,
    layout_string: Option<&str>,
    layout_path: Option<&Path>,
    cwd: PathBuf,
) -> Result<(Option<LayoutInfo>, Config), ConfigError> {
    load_layout_and_config_with(
        cli_config_options,
        config,
        config_dir,
        config_path,
        layout_string,
        layout_path,
        cwd,
        ConfigRead::Checked,
    )
}

fn load_layout_and_config_with(
    cli_config_options: Option<&Options>,
    config: Config,
    config_dir: Option<PathBuf>,
    config_path: Option<&Path>,
    layout_string: Option<&str>,
    layout_path: Option<&Path>,
    cwd: PathBuf,
    mode: ConfigRead,
) -> Result<(Option<LayoutInfo>, Config), ConfigError> {
    let layout_dir = configured_layout_dir(cli_config_options, &config, config_dir.clone());
    // Choose the layout source here; the shared layout loader resolves files and builtins.
    let (layout_info, chosen_layout) = if let Some(ref layout_string) = layout_string {
        (
            Some(yinz_types::data::LayoutInfo::Stringified(
                layout_string.to_string(),
            )),
            None,
        )
    } else if let Some(chosen_layout) = layout_path.map(std::path::Path::to_path_buf) {
        let layout_info = crate::layout::layout_info_from_cli_with(
            &layout_dir,
            &Some(chosen_layout.clone()),
            cwd,
            mode,
        );
        (layout_info, Some(chosen_layout))
    } else {
        let chosen_layout = cli_config_options
            .as_ref()
            .and_then(|cli_options| cli_options.default_layout.clone())
            .or_else(|| config.options.default_layout.clone());
        let layout_info =
            crate::layout::layout_info_from_config_with(&layout_dir, &chosen_layout, mode)?;
        (layout_info, chosen_layout)
    };
    match layout_info {
        Some(yinz_types::data::LayoutInfo::Stringified(ref raw_layout)) => {
            crate::layout::from_stringified_layout_with(raw_layout, config, mode)
                .map(|(_layout, config)| (layout_info, config))
        },
        _ => crate::layout::from_path_or_default_with(
            chosen_layout.as_ref(),
            layout_dir,
            config,
            mode,
        )
        .map(|(_layout, config)| (layout_info, config)),
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
    apply_themes_with(config, cli_config_options, config_dir, ConfigRead::Ordinary)
}

/// Load external themes for a checked runtime configuration.
pub fn apply_checked_themes(
    config: &mut Config,
    cli_config_options: Option<Options>,
    config_dir: Option<PathBuf>,
) -> Result<Options, ConfigError> {
    apply_themes_with(config, cli_config_options, config_dir, ConfigRead::Checked)
}

pub(crate) fn apply_themes_with(
    config: &mut Config,
    cli_config_options: Option<Options>,
    config_dir: Option<PathBuf>,
    mode: ConfigRead,
) -> Result<Options, ConfigError> {
    let config_options = match cli_config_options {
        Some(cli_config_options) => config.options.merge(cli_config_options),
        None => config.options.clone(),
    };

    config.themes = crate::assets::get_default_themes().merge(std::mem::take(&mut config.themes));

    let user_theme_dir = match config_options.theme_dir.clone() {
        Some(dir) => Some(dir),
        None => match home::get_theme_dir(config_dir.or_else(home::find_default_config_dir)) {
            Some(dir) if mode.exists(&dir)? => Some(dir),
            _ => None,
        },
    };
    if let Some(user_theme_dir) = user_theme_dir {
        config.themes = config
            .themes
            .merge(Themes::from_dir_with(user_theme_dir, mode.reader())?);
    }
    Ok(config_options)
}

#[cfg(test)]
fn captured_config_logs() -> &'static yinz_test_support::Watched<Vec<String>> {
    use std::sync::LazyLock;
    use yinz_test_support::Watched;

    struct ConfigLogger;
    static LOGS: LazyLock<Watched<Vec<String>>> = LazyLock::new(|| Watched::new(Vec::new()));
    static INSTALL: std::sync::Once = std::sync::Once::new();

    impl log::Log for ConfigLogger {
        fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
            metadata.level() == log::Level::Error && metadata.target() == module_path!()
        }

        fn log(&self, record: &log::Record<'_>) {
            if self.enabled(record.metadata()) {
                LOGS.lock().push(record.args().to_string());
            }
        }

        fn flush(&self) {}
    }

    INSTALL.call_once(|| {
        log::set_logger(&ConfigLogger).unwrap();
        log::set_max_level(log::LevelFilter::Error);
    });
    &LOGS
}

#[cfg(test)]
#[path = "unit/config_config_test.rs"]
mod config_test;

#[cfg(test)]
#[path = "unit/config_overlay_tests.rs"]
mod overlay_tests;

#[cfg(test)]
#[path = "unit/config_kdl_discovery_tests.rs"]
mod kdl_discovery_tests;

#[cfg(test)]
#[path = "unit/config_watcher_tests.rs"]
mod watcher_tests;
