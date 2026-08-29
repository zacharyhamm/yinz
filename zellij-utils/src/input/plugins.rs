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
    /// The path as the user wrote it, when this plugin was named by a relative `file:` URL.
    /// `path` above is absolute by then - the cwd was joined onto it at parse time - so this is
    /// what the folder lookups below have to join instead. See
    /// `RunPluginLocation::parse_with_relative`.
    pub relative_path: Option<PathBuf>,
}

/// Read one candidate Lua source. `Ok(None)` means only that nothing is there, so the search can
/// go on; anything else - a permission denied, a directory where a file was expected - is an
/// error the user needs to see rather than a reason to keep looking and then report the wrong
/// path as missing.
fn read_lua_file(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(source) => Ok(Some(source)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow!("{}", e))
            .with_context(|| format!("failed to read the Lua plugin at '{}'", path.display())),
    }
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
                relative_path: run_plugin.relative_path.clone(),
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
                        // A builtin is named by a tag, not by a path, so there is no relative
                        // form of it to keep.
                        relative_path: None,
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
                relative_path: None,
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

        // Locations we check for valid plugins. The folder lookups join `relative_path` when
        // there is one, for the reason spelled out on `resolve_lua_source`: `path` has had the
        // cwd joined onto it, and joining an absolute path onto a folder is a no-op, so
        // `file:tab-bar.wasm` would otherwise never be looked for in a plugin folder at all.
        let lookup_name = self.relative_path.as_ref().unwrap_or(&self.path);
        #[allow(unused_mut)]
        let mut paths: Vec<PathBuf> = vec![
            self.path.clone(),
            self.path.with_extension("wasm"),
            plugin_dir.join(lookup_name).with_extension("wasm"),
        ];
        #[cfg(not(target_family = "wasm"))]
        paths.push(
            crate::home::system_data_dir()
                .join("plugins")
                .join(lookup_name)
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

    /// Read a Lua plugin's source from disk, looking in the user's plugin folder for anything
    /// not found at the path as given.
    ///
    /// The path is tried first and unchanged, so every existing declaration keeps working and an
    /// absolute path always wins. Only when that is not there does the user plugin dir come into
    /// it, which is what makes `file:keys.lua` - and `file:keys` - mean the obvious thing.
    ///
    /// The fallback keys on "the file is not there" rather than on "the path is relative",
    /// because the path itself is absolute by the time we are called: `RunPluginLocation::parse`
    /// joins a relative `file:` path onto the cwd whenever one is supplied. What the folder is
    /// searched for is `relative_path` - the name as the user wrote it - so `file:keys.lua` looks
    /// for `keys.lua` there rather than for `$cwd/keys.lua`, which could never be found. A path
    /// the user gave in full has no `relative_path`, and so has no second place to be: it must
    /// fail rather than resolve to a same-named plugin the user did not ask for.
    ///
    /// The extension is *not* appended for a bare `file:keys`, tempting as that is: `runtime()`
    /// decides Lua by the `.lua` extension, so an extensionless path is taken for wasm and never
    /// arrives here at all. Appending it would be dead code that reads as a working feature.
    ///
    /// The path that was read comes back alongside the source, because for `file:keys.lua` it is
    /// *not* `self.path` and nothing else records which of the two candidates won. A reload names
    /// the file in full, so that path - not the location, which is the cwd-joined identity - is
    /// what `all_plugin_ids_for_plugin_location` has to match it against.
    pub fn resolve_lua_source(&self, user_plugin_dir: Option<&Path>) -> Result<(PathBuf, String)> {
        if let Some(source) = self.builtin_lua_source() {
            // A builtin has no file at all; its `path` is the bare tag. Handing it back keeps the
            // signature honest, and nothing matches a builtin by file path.
            return Ok((self.path.clone(), source.to_owned()));
        }

        let mut tried = vec![self.path.clone()];
        if let Some(source) = read_lua_file(&self.path)? {
            return Ok((self.path.clone(), source));
        }

        if let Some(dir) = user_plugin_dir {
            // `join` on an absolute path yields the absolute path, so this is a no-op for one
            // that was already tried above rather than a second read of the same file.
            let in_dir = dir.join(self.relative_path.as_ref().unwrap_or(&self.path));
            if in_dir != self.path {
                tried.push(in_dir.clone());
                if let Some(source) = read_lua_file(&in_dir)? {
                    return Ok((in_dir, source));
                }
            }
        }

        let tried = tried
            .iter()
            .map(|path| format!("'{}'", path.display()))
            .collect::<Vec<_>>()
            .join(", ");
        Err(anyhow!(
            "failed to find the Lua plugin '{}'; looked in: {}",
            self.path.display(),
            tried
        ))
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

    fn file_plugin(path: &str) -> PluginConfig {
        PluginConfig::from_run_plugin(&RunPlugin {
            location: RunPluginLocation::File(std::path::PathBuf::from(path)),
            ..Default::default()
        })
        .unwrap()
    }

    /// Why `resolve_lua_source` does not append `.lua`: a path without it is not a Lua plugin
    /// in the first place, so `file:keys` never reaches the Lua loader however the folder is
    /// searched. The extension is the whole of how the runtime is chosen.
    #[test]
    fn an_extensionless_path_is_not_lua_however_it_resolves() {
        assert_eq!(file_plugin("keys").runtime(), super::PluginRuntime::Wasm);
    }

    #[test]
    fn a_lua_plugin_is_found_in_the_user_plugin_dir_by_name() {
        let plugin_dir = tempdir().unwrap();
        fs::write(plugin_dir.path().join("keys.lua"), "-- from the dir").unwrap();

        assert_eq!(
            file_plugin("keys.lua")
                .resolve_lua_source(Some(plugin_dir.path()))
                .unwrap(),
            (
                plugin_dir.path().join("keys.lua"),
                "-- from the dir".to_owned()
            )
        );
    }

    /// A plugin as the `zellij plugin -- file:keys.lua` path builds one: the URL was relative, so
    /// the cwd has already been joined onto it and the path is absolute by now. Only
    /// `relative_path` still knows the plugin was named `keys.lua`, and without it the folder
    /// would be searched for `$cwd/keys.lua` - which `Path::join` returns unchanged, so nothing
    /// in the folder could ever match.
    #[test]
    fn a_relative_url_launched_from_elsewhere_is_found_in_the_user_plugin_dir() {
        let plugin_dir = tempdir().unwrap();
        fs::write(plugin_dir.path().join("keys.lua"), "-- from the dir").unwrap();
        let cwd = tempdir().unwrap();

        let (location, relative_path) =
            RunPluginLocation::parse_with_relative("file:keys.lua", Some(cwd.path().to_owned()))
                .unwrap();
        assert!(
            matches!(&location, RunPluginLocation::File(path) if path.is_absolute()),
            "the cwd should already have been joined on: {location:?}"
        );
        let plugin = PluginConfig::from_run_plugin(&RunPlugin {
            location,
            relative_path,
            ..Default::default()
        })
        .unwrap();

        assert_eq!(
            plugin.resolve_lua_source(Some(plugin_dir.path())).unwrap(),
            (
                plugin_dir.path().join("keys.lua"),
                "-- from the dir".to_owned()
            )
        );
    }

    /// The other half of the same rule, and the reason relativity is carried rather than the
    /// file name simply being retried: a path the user gave in full names one file. If it is not
    /// there, that is the answer - loading a same-named plugin out of the user's folder instead
    /// would run something they did not ask for.
    #[test]
    fn an_absolute_url_that_is_missing_does_not_fall_back_to_the_user_plugin_dir() {
        let plugin_dir = tempdir().unwrap();
        fs::write(plugin_dir.path().join("keys.lua"), "-- from the dir").unwrap();
        let elsewhere = tempdir().unwrap();
        let missing = elsewhere.path().join("keys.lua");

        let (location, relative_path) = RunPluginLocation::parse_with_relative(
            &format!("file:{}", missing.display()),
            Some(elsewhere.path().to_owned()),
        )
        .unwrap();
        assert_eq!(relative_path, None, "the URL was absolute");
        let plugin = PluginConfig::from_run_plugin(&RunPlugin {
            location,
            relative_path,
            ..Default::default()
        })
        .unwrap();

        let error = plugin
            .resolve_lua_source(Some(plugin_dir.path()))
            .unwrap_err()
            .to_string();
        assert!(error.contains("keys.lua"), "{error}");
    }

    /// The path as given always wins, so an existing declaration cannot start meaning a
    /// different file the day a same-named plugin appears in the folder.
    #[test]
    fn a_path_that_exists_beats_the_user_plugin_dir() {
        let plugin_dir = tempdir().unwrap();
        fs::write(plugin_dir.path().join("keys.lua"), "-- from the dir").unwrap();
        let elsewhere = tempdir().unwrap();
        let explicit = elsewhere.path().join("keys.lua");
        fs::write(&explicit, "-- from the explicit path").unwrap();

        assert_eq!(
            file_plugin(&explicit.display().to_string())
                .resolve_lua_source(Some(plugin_dir.path()))
                .unwrap(),
            (explicit, "-- from the explicit path".to_owned())
        );
    }

    #[test]
    fn a_builtin_beats_a_same_named_file_in_the_user_plugin_dir() {
        let plugin_dir = tempdir().unwrap();
        fs::write(plugin_dir.path().join("agent-exit.lua"), "-- impostor").unwrap();

        let (_path, source) = builtin_plugin("agent-exit")
            .resolve_lua_source(Some(plugin_dir.path()))
            .unwrap();
        assert!(
            source.contains("Agent exited."),
            "the compiled-in builtin must not be shadowed by a file"
        );
    }

    /// A typo is much harder to diagnose if the error names only one of the places looked in.
    #[test]
    fn a_missing_lua_plugin_names_every_path_tried() {
        let plugin_dir = tempdir().unwrap();
        let error = file_plugin("nope.lua")
            .resolve_lua_source(Some(plugin_dir.path()))
            .unwrap_err()
            .to_string();

        assert!(error.contains("nope.lua"), "{error}");
        assert!(
            error.contains(&plugin_dir.path().display().to_string()),
            "the folder that was searched should be named too: {error}"
        );
    }

    #[test]
    fn a_lua_plugin_resolves_with_no_user_plugin_dir_at_all() {
        let elsewhere = tempdir().unwrap();
        let explicit = elsewhere.path().join("keys.lua");
        fs::write(&explicit, "-- standalone").unwrap();

        assert_eq!(
            file_plugin(&explicit.display().to_string())
                .resolve_lua_source(None)
                .unwrap(),
            (explicit, "-- standalone".to_owned())
        );
    }

    #[test]
    fn a_builtin_lua_plugin_resolves_to_the_lua_runtime() {
        // A builtin's `path` is the bare tag with no extension, so the extension rule that
        // identifies every other Lua plugin cannot see this one. Every entry in the table,
        // so that a shipped plugin whose name is misspelt in the table is caught here rather
        // than by a user binding it.
        for (name, _) in zellij_utils_consts_builtin_lua_plugins() {
            let plugin = builtin_plugin(name);
            assert_eq!(plugin.runtime(), super::PluginRuntime::Lua, "{name}");
            assert!(
                plugin.is_builtin(),
                "{name}: so that it bypasses permission prompts"
            );
        }
    }

    fn zellij_utils_consts_builtin_lua_plugins() -> &'static [(&'static str, &'static str)] {
        crate::consts::BUILTIN_LUA_PLUGINS
    }

    #[test]
    fn a_builtin_lua_plugin_serves_its_compiled_in_source() {
        let (_path, source) = builtin_plugin("agent-exit")
            .resolve_lua_source(None)
            .unwrap();
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
