//! Plugins configuration metadata
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

use serde::{Deserialize, Serialize};
use url::Url;

use super::layout::{PluginUserConfiguration, RunPlugin, RunPluginLocation};
#[cfg(not(target_family = "wasm"))]
use crate::consts::BUILTIN_PLUGIN_DEV_DIRS;
use crate::consts::{builtin_lua_plugin, BUILTIN_PLUGIN_NAMES};
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
/// The runtime a plugin runs on. Zellij runs plugins compiled to wasm and plugins written in
/// Lua; see `PluginConfig::runtime`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginRuntime {
    Wasm,
    Lua,
}

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
                // A builtin is either a wasm plugin with bytes to find or a Lua plugin with
                // source compiled in; both are named the same way, so both are accepted here.
                if BUILTIN_PLUGIN_NAMES.contains(&tag.as_str())
                    || builtin_lua_plugin(&tag).is_some()
                {
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
    /// Nothing is embedded in the binary - plugin wasm always comes from disk.
    ///
    /// If `builtin_plugins_dir` is set, builtin plugins (starting with 'zellij:' in the layout
    /// file) are loaded from that directory when present. Otherwise we attempt to resolve the
    /// plugin path as an absolute path, then with a ".wasm" extension appended, then the plugin
    /// directory joined with the path plus ".wasm", then the system data directory joined with
    /// "plugins" and the same file name. Builtin plugins finally fall back to
    /// `BUILTIN_PLUGIN_DEV_DIRS`, the in-checkout locations baked in at compile time, so a dev
    /// build or `cargo test` works with no configuration.
    ///
    /// So if our path is "tab-bar" and the given plugin dir is
    /// "/home/bob/.local/share/zellij/plugins" the lookup chain will be this:
    ///
    /// ```bash
    ///   tab-bar
    ///   tab-bar.wasm
    ///   /home/bob/.local/share/zellij/plugins/tab-bar.wasm
    ///   /usr/share/zellij/plugins/tab-bar.wasm
    ///   <checkout>/target/wasm32-wasip1/debug/tab-bar.wasm  (debug builds only)
    ///   <checkout>/zellij-utils/assets/plugins/tab-bar.wasm
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
        #[cfg(not(target_family = "wasm"))]
        if self.is_builtin() {
            paths.extend(
                BUILTIN_PLUGIN_DEV_DIRS
                    .iter()
                    .map(|dir| Path::new(dir).join(&self.path).with_extension("wasm")),
            );
        }
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

            if self.is_builtin_name() {
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

    /// Which runtime should run this plugin, decided by its file extension.
    ///
    /// A `.lua` file is a Lua plugin; anything else is wasm, which keeps every existing plugin
    /// declaration working untouched and means no new URL scheme is needed - `file:~/thing.lua`
    /// is enough.
    pub fn runtime(&self) -> PluginRuntime {
        // A builtin has no extension to read - its `path` is the bare tag - so being in the
        // builtin Lua table is what identifies it.
        if self.builtin_lua_source().is_some() {
            return PluginRuntime::Lua;
        }
        match self.path.extension().and_then(|e| e.to_str()) {
            Some("lua") => PluginRuntime::Lua,
            _ => PluginRuntime::Wasm,
        }
    }

    /// The compiled-in source, if this is a builtin Lua plugin.
    fn builtin_lua_source(&self) -> Option<&'static str> {
        match &self.location {
            RunPluginLocation::Zellij(tag) => builtin_lua_plugin(&tag.to_string()),
            _ => None,
        }
    }

    /// Read a Lua plugin's source from disk. The path is used as given - unlike wasm plugins,
    /// which are searched for across several directories with an extension appended, a Lua plugin
    /// is named by a path that already points at a file.
    pub fn resolve_lua_source(&self) -> Result<String> {
        if let Some(source) = self.builtin_lua_source() {
            return Ok(source.to_owned());
        }
        fs::read_to_string(&self.path)
            .map_err(|e| anyhow!("{}", e))
            .with_context(|| format!("failed to read the Lua plugin at '{}'", self.path.display()))
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
    fn a_builtin_lua_plugin_resolves_to_the_lua_runtime() {
        // A builtin's `path` is the bare tag with no extension, so the extension rule that
        // identifies every other Lua plugin cannot see this one.
        let plugin = builtin_plugin("agent-exit");
        assert_eq!(plugin.runtime(), super::PluginRuntime::Lua);
        assert!(
            plugin.is_builtin(),
            "so that it bypasses permission prompts"
        );
    }

    #[test]
    fn a_builtin_lua_plugin_serves_its_compiled_in_source() {
        let source = builtin_plugin("agent-exit").resolve_lua_source().unwrap();
        assert!(
            source.contains("Agent exited."),
            "the source comes from the embedded asset, not from disk"
        );
    }

    #[test]
    fn an_ordinary_builtin_is_still_wasm() {
        assert_eq!(
            builtin_plugin("status-bar").runtime(),
            super::PluginRuntime::Wasm
        );
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

    #[test]
    fn missing_configured_builtin_plugin_falls_back_to_repo_assets_dir() {
        let plugin_dir = tempdir().unwrap();
        let builtin_plugins_dir = tempdir().unwrap();
        let plugin = builtin_plugin("tab-bar");

        assert!(!plugin
            .resolve_wasm_bytes(plugin_dir.path(), Some(builtin_plugins_dir.path()))
            .unwrap()
            .is_empty());
    }

    // The plugin dir comes from `--data-dir`, which is how the e2e suite supplies builtin
    // plugins. Unique content proves it wins over the in-checkout dev dirs rather than just
    // happening to resolve.
    #[test]
    fn plugin_dir_takes_precedence_over_the_checkout() {
        let plugin_dir = tempdir().unwrap();
        fs::write(plugin_dir.path().join("tab-bar.wasm"), b"from-data-dir").unwrap();

        let plugin = builtin_plugin("tab-bar");
        assert_eq!(
            plugin.resolve_wasm_bytes(plugin_dir.path(), None).unwrap(),
            b"from-data-dir"
        );
    }

    // This is the check that fails if the compile-time dev dirs stop resolving, which is what
    // keeps a plain `cargo test`/`cargo run` in a checkout working with no configuration at all.
    #[test]
    fn builtin_plugin_resolves_from_checkout_with_no_configuration() {
        let plugin_dir = tempdir().unwrap();
        let plugin = builtin_plugin("tab-bar");

        assert!(!plugin
            .resolve_wasm_bytes(plugin_dir.path(), None)
            .unwrap()
            .is_empty());
    }
}
