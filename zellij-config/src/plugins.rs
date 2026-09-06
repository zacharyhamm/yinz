//! Plugins configuration metadata
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[cfg(all(not(target_family = "wasm"), debug_assertions))]
use crate::consts::BUILTIN_PLUGIN_DEBUG_DIR;
#[cfg(not(target_family = "wasm"))]
use crate::consts::BUILTIN_PLUGIN_RELEASE_DIR;
use crate::consts::{builtin_lua_plugin, BUILTIN_PLUGIN_NAMES};
use zellij_runtime::errors::prelude::*;
use zellij_types::data::PluginTag;
use {
    zellij_types::input::layout::PluginUserConfiguration, zellij_types::input::layout::RunPlugin,
    zellij_types::input::layout::RunPluginLocation,
};

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
    /// `zellij_runtime::plugin_location::parse_plugin_location_with_relative`.
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
                        location: zellij_runtime::plugin_location::parse_plugin_location(
                            &format!("zellij:{}", tag),
                            None,
                        )
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
    /// A representative builtin, for callers that want to show where builtins are looked for
    /// (`zellij setup --check`) without caring which one.
    pub fn builtin_lookup_example() -> PluginConfig {
        PluginConfig {
            path: PathBuf::from("tab-bar"),
            _allow_exec_host_cmd: false,
            location: RunPluginLocation::Zellij(PluginTag::new("tab-bar")),
            initial_userspace_configuration: Default::default(),
            initial_cwd: None,
            relative_path: None,
        }
    }

    /// The path a builtin would be read from under the configured `builtin_plugins_dir`, if
    /// this is a builtin and the option is set.
    fn configured_builtin_path(&self, builtin_plugins_dir: Option<&Path>) -> Option<PathBuf> {
        if !self.is_builtin() {
            return None;
        }
        builtin_plugins_dir.map(|dir| dir.join(&self.path).with_extension("wasm"))
    }

    /// Every path `resolve_wasm_bytes` would try, in order, given the session's data dir. The
    /// first entry is the `builtin_plugins_dir` candidate when there is one; the rest is the
    /// chain documented on `resolve_wasm_bytes`. Pure, so the order can be tested and so that
    /// `zellij setup --check` prints the very same list rather than a hand-maintained copy.
    pub fn wasm_lookup_paths(
        &self,
        data_dir: &Path,
        builtin_plugins_dir: Option<&Path>,
    ) -> Vec<PathBuf> {
        let plugin_dir = data_dir.join("plugins");
        // The folder lookups join `relative_path` when there is one, for the reason spelled out
        // on `resolve_lua_source`: `path` has had the cwd joined onto it, and joining an
        // absolute path onto a folder is a no-op, so `file:tab-bar.wasm` would otherwise never
        // be looked for in a plugin folder at all.
        let lookup_name = self.relative_path.as_ref().unwrap_or(&self.path);
        let mut paths: Vec<PathBuf> = self
            .configured_builtin_path(builtin_plugins_dir)
            .into_iter()
            .collect();
        paths.push(self.path.clone());
        paths.push(self.path.with_extension("wasm"));
        paths.push(plugin_dir.join(lookup_name).with_extension("wasm"));
        #[cfg(not(target_family = "wasm"))]
        {
            let builtin_name = self.path.with_extension("wasm");
            if self.is_builtin() {
                // Fresh debug plugins first, so a checkout on the same VERSION as an install
                // sees what it just built.
                #[cfg(debug_assertions)]
                paths.push(Path::new(BUILTIN_PLUGIN_DEBUG_DIR).join(&builtin_name));
                // What `cargo x install` wrote - for the user, then system-wide. These two sit
                // next to each other so that `dedup` below collapses them when the default data
                // dir fell back to the system one.
                paths.push(zellij_runtime::home::builtin_plugin_dir(data_dir).join(&builtin_name));
                paths.push(zellij_runtime::home::system_builtin_plugin_dir().join(&builtin_name));
            }
            paths.push(
                zellij_runtime::home::system_data_dir()
                    .join("plugins")
                    .join(lookup_name)
                    .with_extension("wasm"),
            );
            if self.is_builtin() {
                paths.push(Path::new(BUILTIN_PLUGIN_RELEASE_DIR).join(&builtin_name));
            }
        }
        // Throw out dupes, because it's confusing to read that zellij checked the same plugin
        // location multiple times. Do NOT sort the vector here, because it will break the lookup!
        paths.dedup();
        paths
    }

    /// Resolve wasm plugin bytes for the plugin path, given the session's data directory.
    ///
    /// Nothing is embedded in the binary - plugin wasm always comes from disk.
    ///
    /// If `builtin_plugins_dir` is set, builtin plugins (`zellij:` URLs) are loaded from that
    /// directory when present. Otherwise the plugin path is tried as given, then with a
    /// `.wasm` extension, then in the data dir's `plugins` folder. Builtins then go through
    /// the checkout's debug plugins (debug binaries only), the builtins that `cargo x install`
    /// put under the data dir and under the system data dir for this `VERSION`, the system
    /// `plugins` folder, and finally the checkout's release plugins - the last two baked in
    /// at compile time so a dev build or `cargo test` works with no configuration, and
    /// harmlessly absent on an installed system.
    ///
    /// So if our path is "tab-bar" and the data dir is "/home/bob/.local/share/zellij", the
    /// lookup chain of a debug binary is:
    ///
    /// ```bash
    ///   tab-bar
    ///   tab-bar.wasm
    ///   /home/bob/.local/share/zellij/plugins/tab-bar.wasm
    ///   <checkout>/target/wasm32-wasip1/debug/tab-bar.wasm         (debug builds only)
    ///   /home/bob/.local/share/zellij/builtins/<VERSION>/tab-bar.wasm
    ///   /usr/share/zellij/builtins/<VERSION>/tab-bar.wasm
    ///   /usr/share/zellij/plugins/tab-bar.wasm
    ///   <checkout>/target/wasm32-wasip1/release/tab-bar.wasm
    /// ```
    ///
    pub fn resolve_wasm_bytes(
        &self,
        data_dir: &Path,
        builtin_plugins_dir: Option<&Path>,
    ) -> Result<Vec<u8>> {
        let err_context =
            |err: std::io::Error, path: &PathBuf| format!("{}: '{}'", err, path.display());

        // The configured folder is the one place where anything other than "not there" is an
        // error worth stopping for: the user pointed at it on purpose.
        if let Some(builtin_plugin_path) = self.configured_builtin_path(builtin_plugins_dir) {
            match fs::read(&builtin_plugin_path) {
                Ok(bytes) => {
                    log::info!(
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

        // This looks weird and usually we would handle errors like this differently, but in this
        // case it's helpful for users and developers alike. This way we preserve all the lookup
        // errors and can report all of them back. We must initialize `last_err` with something,
        // and since the user will only get to see it when loading a plugin failed, we may as well
        // spell it out right here.
        let mut last_err: Result<Vec<u8>> = Err(anyhow!("failed to load plugin from disk"));
        // `None` here: the configured folder was already tried above, with stricter errors.
        for path in self.wasm_lookup_paths(data_dir, None) {
            // Try to read from disk
            match fs::read(&path) {
                Ok(val) => {
                    // Info, not debug: which of the lookup folders won is the one thing worth
                    // knowing when an install and a checkout both hold a copy.
                    log::info!("Loaded plugin '{}' from disk", path.display());
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
            let plugin_dir = data_dir.join("plugins");

            if self.is_builtin_name() {
                return Err(ZellijError::BuiltinPluginMissing {
                    plugin_path,
                    plugin_dir,
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
    /// because the path itself is absolute by the time we are called: `zellij_runtime::plugin_location::parse_plugin_location`
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

#[cfg(test)]
mod tests {
    use super::{PluginConfig, PluginTag, RunPlugin, RunPluginLocation};
    use std::fs;
    use std::path::PathBuf;
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
            zellij_runtime::plugin_location::parse_plugin_location_with_relative(
                "file:keys.lua",
                Some(cwd.path().to_owned()),
            )
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

        let (location, relative_path) =
            zellij_runtime::plugin_location::parse_plugin_location_with_relative(
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
        for (name, _) in builtin_lua_plugins_for_tests() {
            let plugin = builtin_plugin(name);
            assert_eq!(plugin.runtime(), super::PluginRuntime::Lua, "{name}");
            assert!(
                plugin.is_builtin(),
                "{name}: so that it bypasses permission prompts"
            );
        }
    }

    fn builtin_lua_plugins_for_tests() -> &'static [(&'static str, &'static str)] {
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
        let data_dir = tempdir().unwrap();
        let builtin_plugins_dir = tempdir().unwrap();
        let plugin_path = builtin_plugins_dir.path().join("tab-bar.wasm");
        fs::write(&plugin_path, b"override").unwrap();

        let plugin = builtin_plugin("tab-bar");
        assert_eq!(
            plugin
                .resolve_wasm_bytes(data_dir.path(), Some(builtin_plugins_dir.path()))
                .unwrap(),
            b"override"
        );
    }

    #[test]
    fn missing_configured_builtin_plugin_falls_back_to_the_checkout() {
        let data_dir = tempdir().unwrap();
        let builtin_plugins_dir = tempdir().unwrap();
        let plugin = builtin_plugin("tab-bar");

        assert!(!plugin
            .resolve_wasm_bytes(data_dir.path(), Some(builtin_plugins_dir.path()))
            .unwrap()
            .is_empty());
    }

    // `<data dir>/plugins` comes from `--data-dir`, which is how the e2e suite supplies builtin
    // plugins. Unique content proves it wins over the in-checkout dev dirs rather than just
    // happening to resolve.
    #[test]
    fn the_data_dir_plugins_folder_takes_precedence_over_the_checkout() {
        let data_dir = tempdir().unwrap();
        let plugin_dir = data_dir.path().join("plugins");
        fs::create_dir_all(&plugin_dir).unwrap();
        fs::write(plugin_dir.join("tab-bar.wasm"), b"from-data-dir").unwrap();

        let plugin = builtin_plugin("tab-bar");
        assert_eq!(
            plugin.resolve_wasm_bytes(data_dir.path(), None).unwrap(),
            b"from-data-dir"
        );
    }

    /// What `cargo x install` writes. Found under the data dir handed in, so a `--data-dir`
    /// carries its own builtins and a test never sees the developer's real install.
    #[test]
    fn an_installed_builtin_is_found_under_the_data_dir() {
        let data_dir = tempdir().unwrap();
        let installed = zellij_runtime::home::builtin_plugin_dir(data_dir.path());
        fs::create_dir_all(&installed).unwrap();
        fs::write(installed.join("tab-bar.wasm"), b"installed").unwrap();

        // A release build has no debug dir to shadow the install; a debug build (this test)
        // does, which is the point of the debug dir - so tell them apart by whether the
        // checkout's debug wasm exists.
        #[cfg(debug_assertions)]
        let debug_copy = std::path::Path::new(super::BUILTIN_PLUGIN_DEBUG_DIR).join("tab-bar.wasm");
        #[cfg(not(debug_assertions))]
        let debug_copy = PathBuf::from("/nonexistent/tab-bar.wasm");
        let bytes = builtin_plugin("tab-bar")
            .resolve_wasm_bytes(data_dir.path(), None)
            .unwrap();
        if debug_copy.is_file() {
            assert_eq!(bytes, fs::read(debug_copy).unwrap());
        } else {
            assert_eq!(bytes, b"installed");
        }
    }

    #[test]
    fn the_data_dir_plugins_folder_beats_the_installed_builtin() {
        let data_dir = tempdir().unwrap();
        let plugin_dir = data_dir.path().join("plugins");
        fs::create_dir_all(&plugin_dir).unwrap();
        fs::write(plugin_dir.join("tab-bar.wasm"), b"from-data-dir").unwrap();
        let installed = zellij_runtime::home::builtin_plugin_dir(data_dir.path());
        fs::create_dir_all(&installed).unwrap();
        fs::write(installed.join("tab-bar.wasm"), b"installed").unwrap();

        assert_eq!(
            builtin_plugin("tab-bar")
                .resolve_wasm_bytes(data_dir.path(), None)
                .unwrap(),
            b"from-data-dir"
        );
    }

    /// The whole reason the install dir is versioned: another version's wasm must never be
    /// picked up. The fall-through lands wherever the checkout has a plugin, which may well
    /// succeed - so the assertion is "not those bytes", not "an error".
    #[test]
    fn an_installed_builtin_of_another_version_is_ignored() {
        let data_dir = tempdir().unwrap();
        let other_version = data_dir.path().join("builtins").join("0.0.0");
        fs::create_dir_all(&other_version).unwrap();
        fs::write(other_version.join("tab-bar.wasm"), b"stale").unwrap();

        let bytes = builtin_plugin("tab-bar")
            .resolve_wasm_bytes(data_dir.path(), None)
            .ok();
        assert_ne!(bytes.as_deref(), Some(&b"stale"[..]));
    }

    /// The documented order, which `zellij setup --check` prints verbatim.
    #[test]
    fn the_lookup_order_is_stable() {
        let data_dir = std::path::Path::new("/data");
        let configured = std::path::Path::new("/configured");
        let paths = builtin_plugin("tab-bar").wasm_lookup_paths(data_dir, Some(configured));

        let mut expected = vec![
            PathBuf::from("/configured/tab-bar.wasm"),
            PathBuf::from("tab-bar"),
            PathBuf::from("tab-bar.wasm"),
            PathBuf::from("/data/plugins/tab-bar.wasm"),
        ];
        #[cfg(debug_assertions)]
        expected.push(std::path::Path::new(super::BUILTIN_PLUGIN_DEBUG_DIR).join("tab-bar.wasm"));
        expected.push(zellij_runtime::home::builtin_plugin_dir(data_dir).join("tab-bar.wasm"));
        expected.push(zellij_runtime::home::system_builtin_plugin_dir().join("tab-bar.wasm"));
        expected.push(
            zellij_runtime::home::system_data_dir()
                .join("plugins")
                .join("tab-bar.wasm"),
        );
        expected.push(std::path::Path::new(super::BUILTIN_PLUGIN_RELEASE_DIR).join("tab-bar.wasm"));
        assert_eq!(paths, expected);
    }

    /// A `file:` plugin is not a builtin and must not be looked for in the builtin folders.
    #[test]
    fn a_file_plugin_skips_the_builtin_folders() {
        let paths = file_plugin("/elsewhere/mine.wasm").wasm_lookup_paths(
            std::path::Path::new("/data"),
            Some(std::path::Path::new("/c")),
        );
        assert!(
            paths
                .iter()
                .all(|path| !path.to_string_lossy().contains("builtins")
                    && !path.starts_with("/c")
                    && !path.to_string_lossy().contains("wasm32-wasip1")),
            "{paths:?}"
        );
    }

    // This is the check that fails if the compile-time dev dirs stop resolving, which is what
    // keeps a `cargo x test`/`cargo x run` in a checkout working with no configuration at all.
    // It needs the plugins built - `cargo x build -p` - which `cargo x test` does first.
    #[test]
    fn builtin_plugin_resolves_from_checkout_with_no_configuration() {
        let data_dir = tempdir().unwrap();
        let plugin = builtin_plugin("tab-bar");

        assert!(!plugin
            .resolve_wasm_bytes(data_dir.path(), None)
            .unwrap()
            .is_empty());
    }
}
