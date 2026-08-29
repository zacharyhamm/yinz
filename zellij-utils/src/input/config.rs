use crate::data::Styling;

#[cfg(not(target_family = "wasm"))]
use crate::data::{LayoutInfo, LayoutWithError};

use miette::{Diagnostic, LabeledSpan, NamedSource, SourceCode};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use thiserror::Error;

use std::convert::TryFrom;

use super::context_menu::ContextMenu;
use super::hooks::Hooks;
use super::keybinds::Keybinds;
use super::layout::RunPluginOrAlias;
use super::options::Options;
use super::plugins::{PluginAliases, PluginsConfigError};
use super::theme::{Themes, UiConfig};
use crate::cli::{CliArgs, Command};
use crate::envs::EnvironmentVariables;
use crate::{home, setup};

pub const DEFAULT_CONFIG_FILE_NAME: &str = "config.kdl";
pub const DEFAULT_LUA_CONFIG_FILE_NAME: &str = "config.lua";

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
        write!(f, "Failed to parse Zellij configuration")
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
                "For more information, please see our configuration guide: https://zellij.dev/documentation/configuration.html".to_string(),
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

#[derive(Error, Debug, Diagnostic)]
pub enum ConfigError {
    // Deserialization error
    #[error("Deserialization error: {0}")]
    KdlDeserializationError(#[from] kdl::KdlError),
    #[error("KdlDeserialization error: {0}")]
    KdlError(KdlError), // TODO: consolidate these
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
    #[error("{0}")]
    DownloadError(String),
    #[error("failed to block on async task")]
    Async(#[from] std::io::Error),
    #[error("Lua configuration error: {0}")]
    Lua(String),
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
                "For more information, please see our layout guide: https://zellij.dev/documentation/creating-a-layout.html".to_string(),
            ),
        })
    }
}

#[derive(Debug, Error)]
pub enum ConversionError {
    #[error("{0}")]
    UnknownInputMode(String),
}

/// Merge `fragment`'s top-level nodes into `managed`: a node replaces the one of the same
/// name, unless both have children, in which case the children are merged by name, one level
/// deep. See `Config::write_managed_fragment`.
pub fn merge_kdl_fragment(managed: &mut kdl::KdlDocument, fragment: &kdl::KdlDocument) {
    for node in fragment.nodes() {
        let name = node.name().value();
        let existing = managed
            .nodes_mut()
            .iter_mut()
            .find(|candidate| candidate.name().value() == name);
        match existing {
            Some(target) if node.children().is_some() && target.children().is_some() => {
                let target_children = target.ensure_children();
                for child in node.children().map(|c| c.nodes()).unwrap_or_default() {
                    let child_name = child.name().value();
                    match target_children
                        .nodes_mut()
                        .iter_mut()
                        .find(|candidate| candidate.name().value() == child_name)
                    {
                        Some(slot) => *slot = child.clone(),
                        None => target_children.nodes_mut().push(child.clone()),
                    }
                }
            },
            Some(target) => *target = node.clone(),
            None => managed.nodes_mut().push(node.clone()),
        }
    }
}

impl TryFrom<&CliArgs> for Config {
    type Error = ConfigError;

    fn try_from(opts: &CliArgs) -> ConfigResult {
        if let Some(Command::Setup(ref setup)) = opts.command {
            if setup.clean {
                return Config::from_default_assets();
            }
        }

        // The config *directory*, whichever file is read: the drop-ins hang off it, so a
        // one-off `--config /somewhere/else.kdl` still gets the user's `config.d`, and never
        // `/somewhere/config.d`.
        let config_dir = opts
            .config_dir
            .clone()
            .or_else(home::find_default_config_dir);
        let overlay_dir = home::get_config_overlay_dir(config_dir.clone());

        if let Some(ref path) = opts.config {
            let default_config = Config::from_default_assets()?;
            return Config::from_path(path, Some(default_config))
                .map(|config| config.with_overlay_dir(overlay_dir.as_deref()));
        }

        let config = if let Some(ref config) = config_dir {
            let lua_path = config.join(DEFAULT_LUA_CONFIG_FILE_NAME);
            let path = if lua_path.exists() {
                lua_path
            } else {
                config.join(DEFAULT_CONFIG_FILE_NAME)
            };
            if path.exists() {
                let default_config = Config::from_default_assets()?;
                Config::from_path(&path, Some(default_config))?
            } else {
                Config::from_default_assets()?
            }
        } else {
            Config::from_default_assets()?
        };
        Ok(config.with_overlay_dir(overlay_dir.as_deref()))
    }
}

/// The drop-in folder under the config dir. See `home::get_config_overlay_dir`.
pub const CONFIG_OVERLAY_DIR_NAME: &str = "config.d";
/// The drop-in zellij writes itself, for anything it is asked to persist: `zellij action
/// reconfigure --write`, `zellij action set-hook --write`, a plugin's `reconfigure(kdl, true)`.
/// A person's own config file - KDL or Lua - is never rewritten for those.
pub const MANAGED_CONFIG_FILE_NAME: &str = "zellij-managed.kdl";

impl Config {
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
        paths.into_iter().fold(self, |config, path| {
            let text = match std::fs::read_to_string(&path) {
                Ok(text) => text,
                Err(e) => {
                    log::error!("Skipping config drop-in {}: {}", path.display(), e);
                    return config;
                },
            };
            match Config::from_kdl(&text, Some(config.clone())) {
                Ok(layered) => layered,
                Err(e) => {
                    log::error!("Skipping config drop-in {}: {}", path.display(), e);
                    config
                },
            }
        })
    }

    /// Persist a KDL `fragment` into `<overlay_dir>/zellij-managed.kdl`, and answer the path.
    ///
    /// The fragment is merged into what the file already holds by node: a top-level node
    /// replaces the one of the same name - `mouse_mode false` sets that one option - except
    /// that where both have children the children are merged by name, one level deep, so
    /// `hooks { agent_exit "" }` unbinds one point and leaves the others. A `keybinds` mode
    /// block is a child, so it is replaced whole. The merged file is parsed over the
    /// defaults before anything is written, and the write is atomic, so the file on disk is
    /// always one the loader accepts.
    pub fn write_managed_fragment(overlay_dir: &Path, fragment: &str) -> Result<PathBuf, String> {
        use kdl::KdlDocument;
        let path = overlay_dir.join(MANAGED_CONFIG_FILE_NAME);
        let fragment: KdlDocument = fragment
            .parse()
            .map_err(|e: kdl::KdlError| format!("the config fragment is not valid KDL: {e}"))?;
        let mut managed: KdlDocument = match std::fs::read_to_string(&path) {
            Ok(text) => text.parse().map_err(|e: kdl::KdlError| {
                format!(
                    "{} is not valid KDL; fix or remove it before zellij can write to it: {e}",
                    path.display()
                )
            })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => KdlDocument::new(),
            Err(e) => return Err(format!("failed to read {}: {e}", path.display())),
        };
        merge_kdl_fragment(&mut managed, &fragment);
        let text = managed.to_string();
        let defaults = Config::from_default_assets().map_err(|e| e.to_string())?;
        Config::from_kdl(&text, Some(defaults))
            .map_err(|e| format!("the merged {} would not load: {e}", path.display()))?;
        std::fs::create_dir_all(overlay_dir)
            .map_err(|e| format!("failed to create {}: {e}", overlay_dir.display()))?;
        let tmp = overlay_dir.join(format!(
            ".{MANAGED_CONFIG_FILE_NAME}.{}",
            std::process::id()
        ));
        std::fs::write(&tmp, &text)
            .map_err(|e| format!("failed to write {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &path)
            .map_err(|e| format!("failed to replace {}: {e}", path.display()))?;
        Ok(path)
    }

    pub fn theme_config(&self, theme_name: Option<&String>) -> Option<Styling> {
        match &theme_name {
            Some(theme_name) => self.themes.get_theme(theme_name).map(|theme| theme.palette),
            None => self.themes.get_theme("default").map(|theme| theme.palette),
        }
    }
    /// Gets default configuration from assets
    #[allow(clippy::result_large_err)]
    pub fn from_default_assets() -> ConfigResult {
        let cfg = String::from_utf8(setup::DEFAULT_CONFIG.to_vec())?;
        match Self::from_kdl(&cfg, None) {
            Ok(config) => Ok(config),
            Err(ConfigError::KdlError(kdl_error)) => Err(ConfigError::KdlError(
                kdl_error.add_src("Default built-in-configuration".into(), cfg),
            )),
            Err(e) => Err(e),
        }
    }
    #[allow(clippy::result_large_err)]
    pub fn from_path(path: &PathBuf, default_config: Option<Config>) -> ConfigResult {
        #[cfg(not(target_family = "wasm"))]
        if path.extension().and_then(|extension| extension.to_str()) == Some("lua") {
            return super::lua_config::from_path(path, default_config);
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
    #[allow(clippy::result_large_err)]
    pub fn merge(&mut self, other: Config) -> Result<(), ConfigError> {
        self.options = self.options.merge(other.options);
        self.keybinds.merge(other.keybinds.clone());
        self.themes = self.themes.merge(other.themes);
        self.plugins.merge(other.plugins);
        self.ui = self.ui.merge(other.ui);
        self.env = self.env.merge(other.env);
        Ok(())
    }
    pub fn config_file_path(opts: &CliArgs) -> Option<PathBuf> {
        opts.config.clone().or_else(|| {
            opts.config_dir
                .clone()
                .or_else(|| {
                    home::try_create_home_config_dir();
                    home::find_default_config_dir()
                })
                .map(|config_dir| {
                    let lua_path = config_dir.join(DEFAULT_LUA_CONFIG_FILE_NAME);
                    if lua_path.exists() {
                        lua_path
                    } else {
                        config_dir.join(DEFAULT_CONFIG_FILE_NAME)
                    }
                })
        })
    }
    pub fn default_config_file_path() -> Option<PathBuf> {
        home::find_default_config_dir().map(|config_dir| {
            let lua_path = config_dir.join(DEFAULT_LUA_CONFIG_FILE_NAME);
            if lua_path.exists() {
                lua_path
            } else {
                config_dir.join(DEFAULT_CONFIG_FILE_NAME)
            }
        })
    }
    pub fn write_config_to_disk(
        config: String,
        config_file_path: &Path,
    ) -> Result<Config, Option<PathBuf>> {
        // if we fail, try to return the PathBuf of the file we were not able to write to
        let config_file_path = config_file_path.to_path_buf();
        Config::from_kdl(&config, None)
            .map_err(|e| {
                log::error!("Failed to parse config: {}", e);
                None
            })
            .and_then(|parsed_config| {
                let backed_up_file_name = Config::backup_current_config(&config_file_path)?;
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
                std::fs::write(&config_file_path, config.as_bytes()).map_err(|e| {
                    log::error!("Failed to write config: {}", e);
                    Some(config_file_path.clone())
                })?;
                let written_config = std::fs::read_to_string(&config_file_path).map_err(|e| {
                    log::error!("Failed to read written config: {}", e);
                    Some(config_file_path.clone())
                })?;
                let parsed_written_config =
                    Config::from_kdl(&written_config, None).map_err(|e| {
                        log::error!("Failed to parse written config: {}", e);
                        None
                    })?;
                if parsed_written_config == parsed_config {
                    Ok(parsed_config)
                } else {
                    log::error!("Configuration corrupted when writing to disk");
                    Err(Some(config_file_path))
                }
            })
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
        if config_file_path.exists() {
            false
        } else {
            if let Err(e) = std::fs::write(&config_file_path, config.as_bytes()) {
                log::error!("Failed to write config to disk: {}", e);
                return false;
            }
            match std::fs::read_to_string(&config_file_path) {
                Ok(written_config) => written_config == config,
                Err(e) => {
                    log::error!("Failed to read written config: {}", e);
                    false
                },
            }
        }
    }
    fn find_free_backup_file_name(config_file_path: &Path) -> Option<PathBuf> {
        let mut backup_config_path = None;
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
            if !potential_config_path.exists() {
                backup_config_path = Some(potential_config_path);
                break;
            }
        }
        backup_config_path
    }
    fn backup_config_with_written_content_confirmation(
        current_config: &str,
        current_config_file_path: &PathBuf,
        backup_config_path: &PathBuf,
    ) -> bool {
        let _ = std::fs::copy(current_config_file_path, backup_config_path);
        match std::fs::read_to_string(backup_config_path) {
            Ok(backed_up_config) => current_config == backed_up_config,
            Err(e) => {
                log::error!(
                    "Failed to back up config file {}: {:?}",
                    backup_config_path.display(),
                    e
                );
                false
            },
        }
    }
    fn backup_current_config(
        config_file_path: &PathBuf,
    ) -> Result<Option<PathBuf>, Option<PathBuf>> {
        // if we fail, try to return the PathBuf of the file we were not able to write to
        // if let Some(config_file_path) = Config::config_file_path(&opts) {
        match std::fs::read_to_string(config_file_path) {
            Ok(current_config) => {
                let Some(backup_config_path) = Config::find_free_backup_file_name(config_file_path)
                else {
                    log::error!("Failed to find a file name to back up the configuration to, ran out of files.");
                    return Err(None);
                };
                if Config::backup_config_with_written_content_confirmation(
                    &current_config,
                    config_file_path,
                    &backup_config_path,
                ) {
                    Ok(Some(backup_config_path))
                } else {
                    log::error!(
                        "Failed to back up config file: {}",
                        backup_config_path.display()
                    );
                    Err(Some(backup_config_path))
                }
            },
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    Ok(None)
                } else {
                    log::error!(
                        "Failed to read current config {}: {}",
                        config_file_path.display(),
                        e
                    );
                    Err(Some(config_file_path.clone()))
                }
            },
        }
    }
    fn autogen_config_message(backed_up_file_name: PathBuf) -> String {
        format!("//\n// THIS FILE WAS AUTOGENERATED BY ZELLIJ, THE PREVIOUS FILE AT THIS LOCATION WAS COPIED TO: {}\n//\n\n", backed_up_file_name.display())
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
    // classic "too many open files" issue if there are a lot of files there and/or lots of Zellij
    // instances
    use crate::setup::Setup;
    use notify::{self, Config as WatcherConfig, Event, PollWatcher, RecursiveMode, Watcher};
    use std::time::Duration;
    use tokio::sync::mpsc;

    fn cli_args_for_config(config_file_path: &Path, config_dir: Option<&Path>) -> CliArgs {
        CliArgs {
            config: Some(config_file_path.to_path_buf()),
            config_dir: config_dir.map(Path::to_path_buf),
            ..Default::default()
        }
    }

    /// The theme dir and the `config.d` drop-in dir, as far as they exist: the two folders a
    /// change in which is a change in the config. Watched together with the file.
    fn load_config_and_theme_dir(
        config_file_path: &Path,
        config_dir: Option<&Path>,
    ) -> Option<(Config, Option<PathBuf>)> {
        let cli_args_for_config = cli_args_for_config(config_file_path, config_dir);
        Setup::from_cli_args(&cli_args_for_config)
            .map(|(config, _, config_options, _, _)| {
                let theme_dir = config_options.theme_dir.or_else(|| {
                    let config_dir = config_dir
                        .map(Path::to_path_buf)
                        .or_else(home::find_default_config_dir);
                    home::get_theme_dir(config_dir).filter(|dir| dir.exists())
                });
                (config, theme_dir)
            })
            .ok()
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
    use crate::input::layout::Layout;
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

                            let (layouts, layout_errors) = Layout::list_available_layouts(
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

#[cfg(test)]
mod config_test {
    use super::*;
    use crate::data::{InputMode, Palette, PaletteColor, StyleDeclaration, Styling};
    use crate::input::actions::Action;
    use crate::input::hooks::HookPoint;
    use crate::input::layout::RunPlugin;
    use crate::input::options::{Clipboard, OnForceClose};
    use crate::input::theme::{FrameConfig, Theme, Themes, UiConfig};
    use std::collections::{BTreeMap, HashMap};
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn try_from_cli_args_with_config() {
        // makes sure loading a config file with --config tries to load the config
        let arbitrary_config = PathBuf::from("nonexistent.yaml");
        let opts = CliArgs {
            config: Some(arbitrary_config),
            ..Default::default()
        };
        println!("OPTS= {:?}", opts);
        let result = Config::try_from(&opts);
        assert!(result.is_err());
    }

    #[test]
    fn try_from_cli_args_with_option_clean() {
        // makes sure --clean works... TODO: how can this actually fail now?
        use crate::setup::Setup;
        let opts = CliArgs {
            command: Some(Command::Setup(Setup {
                clean: true,
                ..Setup::default()
            })),
            ..Default::default()
        };
        let result = Config::try_from(&opts);
        assert!(result.is_ok());
    }

    #[test]
    fn try_from_cli_args_with_config_dir() {
        let mut opts = CliArgs::default();
        let tmp = tempdir().unwrap();
        File::create(tmp.path().join(DEFAULT_CONFIG_FILE_NAME))
            .unwrap()
            .write_all(b"keybinds: invalid\n")
            .unwrap();
        opts.config_dir = Some(tmp.path().to_path_buf());
        let result = Config::try_from(&opts);
        assert!(result.is_err());
    }

    #[test]
    fn try_from_cli_args_with_config_dir_without_config() {
        let mut opts = CliArgs::default();
        let tmp = tempdir().unwrap();
        opts.config_dir = Some(tmp.path().to_path_buf());
        let result = Config::try_from(&opts);
        assert_eq!(result.unwrap(), Config::from_default_assets().unwrap());
    }

    #[test]
    fn try_from_cli_args_default() {
        let opts = CliArgs::default();
        let result = Config::try_from(&opts);
        assert_eq!(result.unwrap(), Config::from_default_assets().unwrap());
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
            Some(String::from("my awesome session")),
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
                sourced_from_external_file: false,
                palette: Styling {
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
                sourced_from_external_file: false,
                palette: Styling {
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
                tab-bar location="zellij:tab-bar"
                status-bar location="zellij:status-bar"
                strider location="zellij:strider"
                compact-bar location="zellij:compact-bar"
                session-manager location="zellij:session-manager"
                welcome-screen location="zellij:session-manager" {
                    welcome_screen true
                }
                filepicker location="zellij:strider"
            }
        "#;
        let config = Config::from_kdl(config_contents, None).unwrap();
        let mut expected_plugin_configuration = BTreeMap::new();
        expected_plugin_configuration.insert(
            "tab-bar".to_owned(),
            RunPlugin::from_url("zellij:tab-bar").unwrap(),
        );
        expected_plugin_configuration.insert(
            "status-bar".to_owned(),
            RunPlugin::from_url("zellij:status-bar").unwrap(),
        );
        expected_plugin_configuration.insert(
            "strider".to_owned(),
            RunPlugin::from_url("zellij:strider").unwrap(),
        );
        expected_plugin_configuration.insert(
            "compact-bar".to_owned(),
            RunPlugin::from_url("zellij:compact-bar").unwrap(),
        );
        expected_plugin_configuration.insert(
            "session-manager".to_owned(),
            RunPlugin::from_url("zellij:session-manager").unwrap(),
        );
        let mut welcome_screen_configuration = BTreeMap::new();
        welcome_screen_configuration.insert("welcome_screen".to_owned(), "true".to_owned());
        expected_plugin_configuration.insert(
            "welcome-screen".to_owned(),
            RunPlugin::from_url("zellij:session-manager")
                .unwrap()
                .with_configuration(welcome_screen_configuration),
        );
        expected_plugin_configuration.insert(
            "filepicker".to_owned(),
            RunPlugin::from_url("zellij:strider").unwrap(),
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
        let round_tripped = Config::from_kdl(&config.to_string(false), None).unwrap();
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
            Some("zellij:context-menu".to_owned())
        );
    }

    #[test]
    fn hooks_bind_a_point_to_a_plugin() {
        let config =
            Config::from_kdl("hooks {\n agent_exit \"file:/tmp/mine.lua\"\n}", None).unwrap();
        assert_eq!(
            config
                .hooks
                .get(HookPoint::AgentExit)
                .map(|p| p.location_string()),
            Some("file:/tmp/mine.lua".to_owned())
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
            Some("zellij:agent-exit".to_owned())
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
        assert!(Config::from_kdl("hooks {\n agent_started \"file:/tmp/x.lua\"\n}", None).is_err());
    }

    #[test]
    fn every_hook_point_binds_from_kdl_and_survives_serialization() {
        // The KDL surface is generic over `HookPoint::ALL`, and this is what keeps it so: a
        // point that can be named can be bound, and comes back bound.
        for point in HookPoint::ALL {
            let kdl = format!("hooks {{\n {point} \"file:/tmp/{point}.lua\"\n}}");
            let config = Config::from_kdl(&kdl, None).unwrap_or_else(|e| panic!("{point}: {e:?}"));
            assert_eq!(
                config.hooks.get(*point).map(|p| p.location_string()),
                Some(format!("file:/tmp/{point}.lua")),
                "{point}"
            );
            let round_tripped = Config::from_kdl(&config.to_string(false), None).unwrap();
            assert_eq!(round_tripped.hooks, config.hooks, "{point}");
        }
    }

    #[test]
    fn hooks_survive_serialization() {
        // Both directions matter: a bound point must come back bound, and an unbound one must
        // stay unbound rather than falling back to the default.
        let bound =
            Config::from_kdl("hooks {\n agent_exit \"file:/tmp/mine.lua\"\n}", None).unwrap();
        let round_tripped = Config::from_kdl(&bound.to_string(false), None).unwrap();
        assert_eq!(round_tripped.hooks, bound.hooks);

        let unbound = Config::from_kdl("hooks {\n agent_exit \"\"\n}", None).unwrap();
        let round_tripped = Config::from_kdl(&unbound.to_string(false), None).unwrap();
        assert!(round_tripped.hooks.get(HookPoint::AgentExit).is_none());

        // A hook is a whole plugin declaration, so its cwd and configuration have to survive
        // too - otherwise the plugin gets a different `load(config)` after a restart.
        let configured = Config::from_kdl(
            r#"
            hooks {
                agent_exit "file:/tmp/mine.lua" {
                    cwd "/tmp/hook-cwd"
                    greeting "hello"
                    loud true
                }
            }
            "#,
            None,
        )
        .unwrap();
        let round_tripped = Config::from_kdl(&configured.to_string(false), None).unwrap();
        assert_eq!(round_tripped.hooks, configured.hooks);
        // And the written form has to be stable, not merely re-readable: `cwd` is parsed into
        // the configuration as well as into the cwd, so a naive writer duplicates it and the
        // copies multiply with every write.
        assert_eq!(
            Config::from_kdl(&round_tripped.to_string(false), None)
                .unwrap()
                .to_string(false),
            round_tripped.to_string(false)
        );
    }

    #[test]
    fn load_plugins_adds_to_defaults_unless_cleared() {
        let base = Config::from_kdl("load_plugins {\n \"zellij:link\"\n}", None).unwrap();
        let names = |config: &Config| -> Vec<String> {
            config
                .background_plugins
                .iter()
                .map(|p| p.location_string())
                .collect()
        };

        let merged =
            Config::from_kdl("load_plugins {\n \"zellij:about\"\n}", Some(base.clone())).unwrap();
        assert_eq!(names(&merged), vec!["zellij:about", "zellij:link"]);

        let cleared = Config::from_kdl(
            "load_plugins clear-defaults=true {\n \"zellij:about\"\n}",
            Some(base),
        )
        .unwrap();
        assert_eq!(names(&cleared), vec!["zellij:about"]);
    }

    #[test]
    fn load_plugins_clear_defaults_survives_serialization() {
        let base = Config::from_kdl("load_plugins {\n \"zellij:link\"\n}", None).unwrap();
        let names = |config: &Config| -> Vec<String> {
            config
                .background_plugins
                .iter()
                .map(|p| p.location_string())
                .collect()
        };

        let cleared = Config::from_kdl(
            "load_plugins clear-defaults=true {\n \"zellij:about\"\n}",
            Some(base.clone()),
        )
        .unwrap();
        let round_tripped =
            Config::from_kdl(&cleared.to_string(false), Some(base.clone())).unwrap();
        assert_eq!(names(&round_tripped), vec!["zellij:about"]);

        // the additive case must not grow a clear-defaults out of nowhere
        let merged =
            Config::from_kdl("load_plugins {\n \"zellij:about\"\n}", Some(base.clone())).unwrap();
        let round_tripped = Config::from_kdl(&merged.to_string(false), Some(base)).unwrap();
        assert_eq!(names(&round_tripped), vec!["zellij:about", "zellij:link"]);
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
            doc("mouse_mode true\ncopy_command \"x\"\nhooks {\n    agent_exit \"file:a.lua\"\n}\n");
        merge_kdl_fragment(
            &mut managed,
            &doc("mouse_mode false\nhooks {\n    context_menu \"\"\n}\nload_plugins {\n    \"file:x.lua\"\n}\n"),
        );
        let text = managed.to_string();
        assert!(text.contains("mouse_mode false"), "{text}");
        assert!(!text.contains("mouse_mode true"), "{text}");
        assert!(
            text.contains("copy_command \"x\""),
            "an untouched option stays: {text}"
        );
        assert!(text.contains("agent_exit \"file:a.lua\""), "{text}");
        assert!(text.contains("context_menu \"\""), "{text}");
        assert!(
            text.contains("load_plugins"),
            "a new block is appended: {text}"
        );
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
            .get(crate::input::hooks::HookPoint::AgentExit)
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
