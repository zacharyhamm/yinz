//! Plugins configuration metadata
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

use serde::{Deserialize, Serialize};
use url::Url;

use super::layout::{PluginUserConfiguration, RunPlugin, RunPluginLocation};
#[cfg(not(target_family = "wasm"))]
use crate::consts::ASSET_MAP;
use crate::consts::BUILTIN_PLUGIN_NAMES;
pub use crate::data::PluginTag;
use crate::errors::prelude::*;

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct PluginAliases {
    pub aliases: BTreeMap<String, RunPlugin>,
}

impl PluginAliases {
    pub fn merge(&mut self, other: Self) {
        self.aliases.extend(other.aliases);
    }
    pub fn from_data(aliases: BTreeMap<String, RunPlugin>) -> Self {
        PluginAliases { aliases }
    }
    pub fn list(&self) -> Vec<String> {
        self.aliases.keys().cloned().collect()
    }
}

/// Plugin metadata
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub struct PluginConfig {
    /// Path of the plugin, see resolve_wasm_bytes for resolution semantics
    pub path: PathBuf,
    /// Allow command execution from plugin
    pub _allow_exec_host_cmd: bool,
    /// Original location of the
    pub location: RunPluginLocation,
    /// Custom configuration for this plugin
    pub initial_userspace_configuration: PluginUserConfiguration,
    /// plugin initial working directory
    pub initial_cwd: Option<PathBuf>,
}

impl PluginConfig {
    /// A "shared" plugin runs as a single wasm instance for the whole session, rather than one
    /// instance per connected client. Opted into with a `shared true` key in the plugin's
    /// configuration block.
    pub fn is_shared(&self) -> bool {
        match self
            .initial_userspace_configuration
            .inner()
            .get("shared")
            .map(|value| value.trim())
        {
            None => false,
            Some(value) => match value.parse::<bool>() {
                Ok(is_shared) => is_shared,
                Err(_) => {
                    log::warn!(
                        "Plugin {}: ignoring unparseable 'shared' configuration value: {:?}",
                        self.path.display(),
                        value
                    );
                    false
                },
            },
        }
    }
    pub fn from_run_plugin(run_plugin: &RunPlugin) -> Option<PluginConfig> {
        match &run_plugin.location {
            RunPluginLocation::File(path) => Some(PluginConfig {
                path: path.clone(),
                _allow_exec_host_cmd: run_plugin._allow_exec_host_cmd,
                location: run_plugin.location.clone(),
                initial_userspace_configuration: run_plugin.configuration.clone(),
                initial_cwd: run_plugin.initial_cwd.clone(),
            }),
            RunPluginLocation::Zellij(tag) => {
                let tag = tag.to_string();
                if BUILTIN_PLUGIN_NAMES.contains(&tag.as_str()) {
                    Some(PluginConfig {
                        path: PathBuf::from(&tag),
                        _allow_exec_host_cmd: run_plugin._allow_exec_host_cmd,
                        location: RunPluginLocation::parse(&format!("zellij:{}", tag), None)
                            .ok()?,
                        initial_userspace_configuration: run_plugin.configuration.clone(),
                        initial_cwd: run_plugin.initial_cwd.clone(),
                    })
                } else {
                    None
                }
            },
            RunPluginLocation::Remote(_) => Some(PluginConfig {
                path: PathBuf::new(),
                _allow_exec_host_cmd: run_plugin._allow_exec_host_cmd,
                location: run_plugin.location.clone(),
                initial_userspace_configuration: run_plugin.configuration.clone(),
                initial_cwd: run_plugin.initial_cwd.clone(),
            }),
        }
    }
    /// Resolve wasm plugin bytes for the plugin path and given plugin directory.
    ///
    /// If `builtin_plugins_dir` is set, builtin plugins (starting with 'zellij:' in the layout
    /// file) are loaded from that directory when present, before falling back to embedded assets.
    /// Otherwise, if zellij was built without the 'disable_automatic_asset_installation' feature,
    /// builtin plugins are loaded directly from the binary-internal asset map. Otherwise:
    ///
    /// Attempts to first resolve the plugin path as an absolute path, then adds a ".wasm"
    /// extension to the path and resolves that, then the plugin directory joined with the path
    /// with an appended ".wasm" extension, and finally the system data directory joined with
    /// "plugins" and the same file name. So if our path is "tab-bar" and the given plugin dir is
    /// "/home/bob/.local/share/zellij/plugins" the lookup chain will be this:
    ///
    /// ```bash
    ///   tab-bar
    ///   tab-bar.wasm
    ///   /home/bob/.local/share/zellij/plugins/tab-bar.wasm
    ///   /usr/share/zellij/plugins/tab-bar.wasm
    /// ```
    ///
    pub fn resolve_wasm_bytes(
        &self,
        plugin_dir: &Path,
        builtin_plugins_dir: Option<&Path>,
    ) -> Result<Vec<u8>> {
        let err_context =
            |err: std::io::Error, path: &PathBuf| format!("{}: '{}'", err, path.display());

        if self.is_builtin() {
            if let Some(builtin_plugins_dir) = builtin_plugins_dir {
                let builtin_plugin_path =
                    builtin_plugins_dir.join(&self.path).with_extension("wasm");
                match fs::read(&builtin_plugin_path) {
                    Ok(bytes) => {
                        log::debug!(
                            "Loaded builtin plugin '{}' from disk",
                            builtin_plugin_path.display()
                        );
                        return Ok(bytes);
                    },
                    Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
                        return Err(anyhow!("{}", err)).with_context(|| {
                            format!(
                                "failed to load builtin plugin from '{}'",
                                builtin_plugin_path.display()
                            )
                        });
                    },
                    Err(_) => {},
                }
            }
        }

        // Locations we check for valid plugins
        #[allow(unused_mut)]
        let mut paths: Vec<PathBuf> = vec![
            self.path.clone(),
            self.path.with_extension("wasm"),
            plugin_dir.join(&self.path).with_extension("wasm"),
        ];
        #[cfg(not(target_family = "wasm"))]
        paths.push(
            crate::home::system_data_dir()
                .join("plugins")
                .join(&self.path)
                .with_extension("wasm"),
        );
        // Throw out dupes, because it's confusing to read that zellij checked the same plugin
        // location multiple times. Do NOT sort the vector here, because it will break the lookup!
        paths.dedup();

        // This looks weird and usually we would handle errors like this differently, but in this
        // case it's helpful for users and developers alike. This way we preserve all the lookup
        // errors and can report all of them back. We must initialize `last_err` with something,
        // and since the user will only get to see it when loading a plugin failed, we may as well
        // spell it out right here.
        let mut last_err: Result<Vec<u8>> = Err(anyhow!("failed to load plugin from disk"));
        for path in paths {
            // Check if the plugin path matches an entry in the asset map. If so, load it directly
            // from memory, don't bother with the disk.
            #[cfg(not(target_family = "wasm"))]
            if !cfg!(feature = "disable_automatic_asset_installation") && self.is_builtin() {
                let asset_path = PathBuf::from("plugins").join(&path);
                if let Some(bytes) = ASSET_MAP.get(&asset_path) {
                    log::debug!("Loaded plugin '{}' from internal assets", path.display());

                    if plugin_dir.join(&path).with_extension("wasm").exists() {
                        log::info!(
                            "Plugin '{}' exists in the 'PLUGIN DIR' at '{}' but is being ignored",
                            path.display(),
                            plugin_dir.display()
                        );
                    }

                    return Ok(bytes.to_vec());
                }
            }

            // Try to read from disk
            match fs::read(&path) {
                Ok(val) => {
                    log::debug!("Loaded plugin '{}' from disk", path.display());
                    return Ok(val);
                },
                Err(err) => {
                    last_err = last_err.with_context(|| err_context(err, &path));
                },
            }
        }

        // Not reached if a plugin is found!
        #[cfg(not(target_family = "wasm"))]
        if self.is_builtin() {
            // Layout requested a builtin plugin that wasn't found
            let plugin_path = self.path.with_extension("wasm");

            if cfg!(feature = "disable_automatic_asset_installation") && self.is_builtin_name() {
                return Err(ZellijError::BuiltinPluginMissing {
                    plugin_path,
                    plugin_dir: plugin_dir.to_owned(),
                    source: last_err.unwrap_err(),
                })
                .context("failed to load a plugin");
            } else {
                return Err(ZellijError::BuiltinPluginNonexistent {
                    plugin_path,
                    source: last_err.unwrap_err(),
                })
                .context("failed to load a plugin");
            }
        }

        last_err
    }

    pub fn is_builtin(&self) -> bool {
        matches!(self.location, RunPluginLocation::Zellij(_))
    }

    pub fn is_builtin_name(&self) -> bool {
        self.path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(|name| BUILTIN_PLUGIN_NAMES.contains(&name))
            .unwrap_or(false)
    }
}

#[derive(Error, Debug, PartialEq)]
pub enum PluginsConfigError {
    #[error("Duplication in plugin tag names is not allowed: '{}'", String::from(.0.clone()))]
    DuplicatePlugins(PluginTag),
    #[error("Failed to parse url: {0:?}")]
    InvalidUrl(#[from] url::ParseError),
    #[error("Only 'file:', 'http(s):' and 'zellij:' url schemes are supported for plugin lookup. '{0}' does not match either.")]
    InvalidUrlScheme(Url),
    #[error("Could not find plugin at the path: '{0:?}'")]
    InvalidPluginLocation(PathBuf),
}

#[cfg(test)]
mod tests {
    use super::{PluginConfig, PluginTag, RunPlugin, RunPluginLocation};
    use std::fs;
    use tempfile::tempdir;

    fn builtin_plugin(name: &str) -> PluginConfig {
        PluginConfig::from_run_plugin(&RunPlugin {
            location: RunPluginLocation::Zellij(PluginTag::new(name)),
            ..Default::default()
        })
        .unwrap()
    }

    #[test]
    fn configured_builtin_plugin_directory_takes_precedence() {
        let plugin_dir = tempdir().unwrap();
        let builtin_plugins_dir = tempdir().unwrap();
        let plugin_path = builtin_plugins_dir.path().join("tab-bar.wasm");
        fs::write(&plugin_path, b"override").unwrap();

        let plugin = builtin_plugin("tab-bar");
        assert_eq!(
            plugin
                .resolve_wasm_bytes(plugin_dir.path(), Some(builtin_plugins_dir.path()))
                .unwrap(),
            b"override"
        );
    }

    #[cfg(not(feature = "disable_automatic_asset_installation"))]
    #[test]
    fn missing_configured_builtin_plugin_falls_back_to_embedded_asset() {
        let plugin_dir = tempdir().unwrap();
        let builtin_plugins_dir = tempdir().unwrap();
        let plugin = builtin_plugin("tab-bar");

        assert!(!plugin
            .resolve_wasm_bytes(plugin_dir.path(), Some(builtin_plugins_dir.path()))
            .unwrap()
            .is_empty());
    }
}
