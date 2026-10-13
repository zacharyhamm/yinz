//! Plugins configuration metadata
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[cfg(all(not(target_family = "wasm"), debug_assertions))]
use crate::consts::BUILTIN_PLUGIN_DEBUG_DIR;
use crate::consts::BUILTIN_PLUGIN_NAMES;
#[cfg(not(target_family = "wasm"))]
use crate::consts::BUILTIN_PLUGIN_RELEASE_DIR;
use yinz_runtime::consts::{BUILTIN_DENO_PLUGIN_NAMES, MAX_DENO_SOURCE_BYTES};
use yinz_runtime::errors::prelude::*;
use yinz_types::data::PluginTag;
use {
    yinz_types::input::layout::PluginUserConfiguration, yinz_types::input::layout::RunPlugin,
    yinz_types::input::layout::RunPluginLocation,
};

/// Plugin metadata
/// The runtime a plugin runs on. Yinz runs plugins compiled to wasm and plugins written in
/// JavaScript/TypeScript; see `PluginConfig::runtime`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginRuntime {
    Wasm,
    Deno,
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
    /// `yinz_runtime::plugin_location::parse_plugin_location_with_relative`.
    pub relative_path: Option<PathBuf>,
    #[serde(default)]
    pub repository_root: Option<PathBuf>,
    #[serde(default)]
    pub installed_commit: Option<String>,
}

/// Read one candidate script source. `Ok(None)` means only that nothing is there, so the search can
/// go on; anything else - a permission denied, a directory where a file was expected - is an
/// error the user needs to see rather than a reason to keep looking and then report the wrong
/// path as missing.
fn read_source_file(path: &Path) -> Result<Option<String>> {
    use std::io::Read;
    let read = || -> std::io::Result<String> {
        let mut source = String::new();
        fs::File::open(path)?
            .take(MAX_DENO_SOURCE_BYTES as u64 + 1)
            .read_to_string(&mut source)?;
        if source.len() > MAX_DENO_SOURCE_BYTES {
            return Err(std::io::Error::other(
                "Deno plugin exceeds 8 MiB source-file budget",
            ));
        }
        Ok(source)
    };
    match read() {
        Ok(source) => Ok(Some(source)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow!("{}", e))
            .with_context(|| format!("failed to read the plugin at '{}'", path.display())),
    }
}

impl PluginConfig {
    /// A "shared" plugin runs as a single instance for the whole session, rather than one
    /// instance per connected client. Opted into with a `shared true` key in the plugin's
    /// configuration block.
    pub fn is_shared(&self) -> bool {
        // Agent launchers must create one pane per action, not one per attached client.
        if matches!(&self.location, RunPluginLocation::Yinz(tag) if matches!(tag.to_string().as_str(), "new-agent" | "agent-opencode"))
        {
            return true;
        }
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
            RunPluginLocation::Git(plugin) => Some(PluginConfig {
                repository_root: None,
                installed_commit: None,
                path: plugin.entrypoint.clone(),
                _allow_exec_host_cmd: run_plugin._allow_exec_host_cmd,
                location: run_plugin.location.clone(),
                initial_userspace_configuration: run_plugin.configuration.clone(),
                initial_cwd: run_plugin.initial_cwd.clone(),
                relative_path: None,
            }),
            RunPluginLocation::File(path) => Some(PluginConfig {
                repository_root: None,
                installed_commit: None,
                path: run_plugin.declared_path.as_ref().unwrap_or(path).clone(),
                _allow_exec_host_cmd: run_plugin._allow_exec_host_cmd,
                location: run_plugin.location.clone(),
                initial_userspace_configuration: run_plugin.configuration.clone(),
                initial_cwd: run_plugin.initial_cwd.clone(),
                relative_path: run_plugin.relative_path.clone(),
            }),
            RunPluginLocation::Yinz(tag) => {
                let tag = tag.to_string();
                // Builtins resolve either to Wasm bytes or TypeScript source the plugin host
                // compiles in.
                if BUILTIN_PLUGIN_NAMES.contains(&tag.as_str())
                    || BUILTIN_DENO_PLUGIN_NAMES.contains(&tag.as_str())
                {
                    Some(PluginConfig {
                        repository_root: None,
                        installed_commit: None,
                        path: PathBuf::from(&tag),
                        _allow_exec_host_cmd: run_plugin._allow_exec_host_cmd,
                        location: yinz_runtime::plugin_location::parse_plugin_location(
                            &format!("yinz:{}", tag),
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
        }
    }
    /// A representative builtin, for callers that want to show where builtins are looked for
    /// (`yinz setup --check`) without caring which one.
    pub fn builtin_lookup_example() -> PluginConfig {
        PluginConfig {
            repository_root: None,
            installed_commit: None,
            path: PathBuf::from("strider"),
            _allow_exec_host_cmd: false,
            location: RunPluginLocation::Yinz(PluginTag::new("strider")),
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
    /// `yinz setup --check` prints the very same list rather than a hand-maintained copy.
    pub fn wasm_lookup_paths(
        &self,
        data_dir: &Path,
        builtin_plugins_dir: Option<&Path>,
    ) -> Vec<PathBuf> {
        let plugin_dir = data_dir.join("plugins");
        // The folder lookups join `relative_path` when there is one, for the reason spelled out
        // on `resolve_source`: `path` has had the cwd joined onto it, and joining an
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
                paths.push(yinz_runtime::home::builtin_plugin_dir(data_dir).join(&builtin_name));
                paths.push(yinz_runtime::home::system_builtin_plugin_dir().join(&builtin_name));
            }
            paths.push(
                yinz_runtime::home::system_data_dir()
                    .join("plugins")
                    .join(lookup_name)
                    .with_extension("wasm"),
            );
            if self.is_builtin() {
                paths.push(Path::new(BUILTIN_PLUGIN_RELEASE_DIR).join(&builtin_name));
            }
        }
        // Throw out dupes, because it's confusing to read that yinz checked the same plugin
        // location multiple times. Do NOT sort the vector here, because it will break the lookup!
        paths.dedup();
        paths
    }

    /// Resolve wasm plugin bytes for the plugin path, given the session's data directory.
    ///
    /// Nothing is embedded in the binary - plugin wasm always comes from disk.
    ///
    /// If `builtin_plugins_dir` is set, builtin plugins (`yinz:` URLs) are loaded from that
    /// directory when present. Otherwise the plugin path is tried as given, then with a
    /// `.wasm` extension, then in the data dir's `plugins` folder. Builtins then go through
    /// the checkout's debug plugins (debug binaries only), the builtins that `cargo x install`
    /// put under the data dir and under the system data dir for this `VERSION`, the system
    /// `plugins` folder, and finally the checkout's release plugins - the last two baked in
    /// at compile time so a dev build or `cargo test` works with no configuration, and
    /// harmlessly absent on an installed system.
    ///
    /// So if our path is "tab-bar" and the data dir is "/home/bob/.local/share/yinz", the
    /// lookup chain of a debug binary is:
    ///
    /// ```bash
    ///   tab-bar
    ///   tab-bar.wasm
    ///   /home/bob/.local/share/yinz/plugins/tab-bar.wasm
    ///   <checkout>/target/wasm32-wasip1/debug/tab-bar.wasm         (debug builds only)
    ///   /home/bob/.local/share/yinz/builtins/<VERSION>/tab-bar.wasm
    ///   /usr/share/yinz/builtins/<VERSION>/tab-bar.wasm
    ///   /usr/share/yinz/plugins/tab-bar.wasm
    ///   <checkout>/target/wasm32-wasip1/release/tab-bar.wasm
    /// ```
    ///
    pub fn resolve_wasm_bytes(
        &self,
        data_dir: &Path,
        builtin_plugins_dir: Option<&Path>,
    ) -> Result<Vec<u8>> {
        self.resolve_wasm_source(data_dir, builtin_plugins_dir)
            .map(|(_, bytes)| bytes)
    }

    /// The selected Wasm source path and its bytes, for source-attributed diagnostics.
    pub fn resolve_wasm_source(
        &self,
        data_dir: &Path,
        builtin_plugins_dir: Option<&Path>,
    ) -> Result<(PathBuf, Vec<u8>)> {
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
                    return Ok((builtin_plugin_path, bytes));
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
        let mut last_err: Result<(PathBuf, Vec<u8>)> =
            Err(anyhow!("failed to load plugin from disk"));
        // `None` here: the configured folder was already tried above, with stricter errors.
        for path in self.wasm_lookup_paths(data_dir, None) {
            // Try to read from disk
            match fs::read(&path) {
                Ok(val) => {
                    // Info, not debug: which of the lookup folders won is the one thing worth
                    // knowing when an install and a checkout both hold a copy.
                    log::info!("Loaded plugin '{}' from disk", path.display());
                    return Ok((path, val));
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
                return Err(YinzError::BuiltinPluginMissing {
                    plugin_path,
                    plugin_dir,
                    source: last_err.unwrap_err(),
                })
                .context("failed to load a plugin");
            } else {
                return Err(YinzError::BuiltinPluginNonexistent {
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
    /// Files with `.js`/`.mjs`/`.ts`/`.mts` extensions use Deno; anything else is wasm.
    /// No new URL scheme is needed: `file:~/thing.ts` is enough.
    pub fn runtime(&self) -> PluginRuntime {
        // Builtin paths are bare tags; their registry selects the script runtime.
        if self.is_builtin_deno() || matches!(self.location, RunPluginLocation::Git(_)) {
            return PluginRuntime::Deno;
        }
        match self.path.extension().and_then(|e| e.to_str()) {
            Some("js" | "mjs" | "ts" | "mts") => PluginRuntime::Deno,
            _ => PluginRuntime::Wasm,
        }
    }

    /// Whether this is one of the TypeScript builtins. Their sources are compiled into the plugin
    /// host from `yinz-deno-builtins`, so all this crate knows of them is their tags, from
    /// `yinz_runtime::consts::BUILTIN_DENO_PLUGIN_NAMES`.
    pub fn is_builtin_deno(&self) -> bool {
        match &self.location {
            RunPluginLocation::Yinz(tag) => {
                BUILTIN_DENO_PLUGIN_NAMES.contains(&tag.to_string().as_str())
            },
            _ => false,
        }
    }

    /// Read a script plugin's source from disk, looking in the user's plugin folder for anything
    /// not found at the path as given.
    ///
    /// The path is tried first and unchanged, so every existing declaration keeps working and an
    /// absolute path always wins. Only when that is not there does the user plugin dir come into
    /// it, which lets `file:keys.ts` resolve in the plugin folder.
    ///
    /// The fallback keys on "the file is not there" rather than on "the path is relative",
    /// because the path itself is absolute by the time we are called: `yinz_runtime::plugin_location::parse_plugin_location`
    /// joins a relative `file:` path onto the cwd whenever one is supplied. What the folder is
    /// searched for is `relative_path` - the name as the user wrote it - so `file:keys.ts` looks
    /// for `keys.ts` there rather than for `$cwd/keys.ts`, which could never be found. A path
    /// the user gave in full has no `relative_path`, and so has no second place to be: it must
    /// fail rather than resolve to a same-named plugin the user did not ask for.
    ///
    /// The extension is *not* appended for a bare `file:keys`, tempting as that is: `runtime()`
    /// selects script runtimes by extension, so an extensionless path is taken for wasm and never
    /// arrives here at all. Appending it would be dead code that reads as a working feature.
    ///
    /// The path that was read comes back alongside the source, because for `file:keys.ts` it is
    /// *not* `self.path` and nothing else records which of the two candidates won. A reload names
    /// the file in full, so that path - not the location, which is the cwd-joined identity - is
    /// what `all_plugin_ids_for_plugin_location` has to match it against.
    ///
    /// A TypeScript builtin is refused. Its source is compiled into the plugin host, which serves
    /// it without asking here, and a builtin has no file at all - its `path` is the bare tag - so
    /// anything found on disk under that name could only be a file posing as it.
    pub fn resolve_source(&self, user_plugin_dir: Option<&Path>) -> Result<(PathBuf, String)> {
        if matches!(self.location, RunPluginLocation::Git(_)) {
            let root = self
                .repository_root
                .as_ref()
                .context("repository plugin was not prepared")?;
            let path = self
                .path
                .canonicalize()
                .context("resolve repository entrypoint")?;
            if !path.starts_with(root) {
                bail!("plugin entrypoint escapes repository");
            }
            return Ok((
                path.clone(),
                read_source_file(&path)?.context("repository entrypoint missing")?,
            ));
        }
        if self.is_builtin_deno() {
            return Err(anyhow!(
                "'{}' is a TypeScript builtin; the plugin host compiles its source in, and no file \
                 may stand in for it",
                self.path.display()
            ));
        }

        let mut tried = vec![self.path.clone()];
        if let Some(source) = read_source_file(&self.path)? {
            return Ok((self.path.clone(), source));
        }

        if let Some(dir) = user_plugin_dir {
            // `join` on an absolute path yields the absolute path, so this is a no-op for one
            // that was already tried above rather than a second read of the same file.
            let in_dir = dir.join(self.relative_path.as_ref().unwrap_or(&self.path));
            if in_dir != self.path {
                tried.push(in_dir.clone());
                if let Some(source) = read_source_file(&in_dir)? {
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
            "failed to find the plugin '{}'; looked in: {}",
            self.path.display(),
            tried
        ))
    }

    pub fn is_builtin(&self) -> bool {
        matches!(self.location, RunPluginLocation::Yinz(_))
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

    #[test]
    fn deno_extensions_share_source_lookup_and_preserve_explicit_paths() {
        let plugins = tempdir().unwrap();
        let cwd = tempdir().unwrap();
        for extension in ["js", "mjs", "ts", "mts"] {
            let name = format!("keys.{extension}");
            let source = "export default {};";
            fs::write(plugins.path().join(&name), source).unwrap();
            let (location, relative_path) =
                yinz_runtime::plugin_location::parse_plugin_location_with_relative(
                    &format!("file:{name}"),
                    Some(cwd.path().to_owned()),
                )
                .unwrap();
            let plugin = PluginConfig::from_run_plugin(&RunPlugin {
                location,
                relative_path,
                ..Default::default()
            })
            .unwrap();
            assert_eq!(plugin.runtime(), super::PluginRuntime::Deno);
            assert_eq!(
                plugin.resolve_source(Some(plugins.path())).unwrap(),
                (plugins.path().join(&name), source.to_owned())
            );
            let explicit = cwd.path().join(&name);
            assert!(file_plugin(explicit.to_str().unwrap())
                .resolve_source(Some(plugins.path()))
                .is_err());
            fs::write(&explicit, "// explicit").unwrap();
            assert_eq!(
                plugin.resolve_source(Some(plugins.path())).unwrap(),
                (explicit, "// explicit".to_owned())
            );
        }
        for name in ["keys", "keys.wasm", "keys.jsx", "keys.tsx", "keys.JS"] {
            assert_eq!(file_plugin(name).runtime(), super::PluginRuntime::Wasm);
        }
    }

    fn builtin_plugin(name: &str) -> PluginConfig {
        PluginConfig::from_run_plugin(&RunPlugin {
            location: RunPluginLocation::Yinz(PluginTag::new(name)),
            ..Default::default()
        })
        .unwrap()
    }

    #[test]
    fn agent_exit_is_client_specific() {
        assert!(!builtin_plugin("agent-exit").is_shared());
    }

    fn file_plugin(path: &str) -> PluginConfig {
        PluginConfig::from_run_plugin(&RunPlugin {
            location: RunPluginLocation::File(std::path::PathBuf::from(path)),
            ..Default::default()
        })
        .unwrap()
    }

    /// Why `resolve_source` does not append `.ts`: a path without it is not a TypeScript plugin
    /// in the first place, so `file:keys` never reaches the TypeScript loader however the folder is
    /// searched. The extension is the whole of how the runtime is chosen.
    #[test]
    fn an_extensionless_path_is_not_a_script_however_it_resolves() {
        assert_eq!(file_plugin("keys").runtime(), super::PluginRuntime::Wasm);
    }

    #[test]
    fn a_script_plugin_is_found_in_the_user_plugin_dir_by_name() {
        let plugin_dir = tempdir().unwrap();
        fs::write(plugin_dir.path().join("keys.ts"), "-- from the dir").unwrap();

        assert_eq!(
            file_plugin("keys.ts")
                .resolve_source(Some(plugin_dir.path()))
                .unwrap(),
            (
                plugin_dir.path().join("keys.ts"),
                "-- from the dir".to_owned()
            )
        );
    }

    /// A plugin as the `yinz plugin -- file:keys.ts` path builds one: the URL was relative, so
    /// the cwd has already been joined onto it and the path is absolute by now. Only
    /// `relative_path` still knows the plugin was named `keys.ts`, and without it the folder
    /// would be searched for `$cwd/keys.ts` - which `Path::join` returns unchanged, so nothing
    /// in the folder could ever match.
    #[test]
    fn a_relative_url_launched_from_elsewhere_is_found_in_the_user_plugin_dir() {
        let plugin_dir = tempdir().unwrap();
        fs::write(plugin_dir.path().join("keys.ts"), "-- from the dir").unwrap();
        let cwd = tempdir().unwrap();

        let (location, relative_path) =
            yinz_runtime::plugin_location::parse_plugin_location_with_relative(
                "file:keys.ts",
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
            plugin.resolve_source(Some(plugin_dir.path())).unwrap(),
            (
                plugin_dir.path().join("keys.ts"),
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
        fs::write(plugin_dir.path().join("keys.ts"), "-- from the dir").unwrap();
        let elsewhere = tempdir().unwrap();
        let missing = elsewhere.path().join("keys.ts");

        let (location, relative_path) =
            yinz_runtime::plugin_location::parse_plugin_location_with_relative(
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
            .resolve_source(Some(plugin_dir.path()))
            .unwrap_err()
            .to_string();
        assert!(error.contains("keys.ts"), "{error}");
    }

    /// The path as given always wins, so an existing declaration cannot start meaning a
    /// different file the day a same-named plugin appears in the folder.
    #[test]
    fn a_path_that_exists_beats_the_user_plugin_dir() {
        let plugin_dir = tempdir().unwrap();
        fs::write(plugin_dir.path().join("keys.ts"), "-- from the dir").unwrap();
        let elsewhere = tempdir().unwrap();
        let explicit = elsewhere.path().join("keys.ts");
        fs::write(&explicit, "-- from the explicit path").unwrap();

        assert_eq!(
            file_plugin(&explicit.display().to_string())
                .resolve_source(Some(plugin_dir.path()))
                .unwrap(),
            (explicit, "-- from the explicit path".to_owned())
        );
    }

    /// The plugin host serves a TypeScript builtin's compiled-in source before it would ever ask
    /// for a file, so a request that reaches here anyway must fail rather than find a same-named
    /// file posing as the builtin.
    #[test]
    fn a_builtin_deno_tag_is_never_read_from_a_file() {
        let plugin_dir = tempdir().unwrap();
        for name in yinz_runtime::consts::BUILTIN_DENO_PLUGIN_NAMES {
            fs::write(plugin_dir.path().join(name), "-- impostor").unwrap();
            fs::write(plugin_dir.path().join(format!("{name}.ts")), "-- impostor").unwrap();
            let plugin = builtin_plugin(name);
            assert_eq!(plugin.runtime(), super::PluginRuntime::Deno);
            assert!(plugin.is_builtin() && plugin.is_builtin_deno());
            let error = plugin
                .resolve_source(Some(plugin_dir.path()))
                .unwrap_err()
                .to_string();
            assert!(error.contains("TypeScript builtin"), "{error}");
        }
        assert!(!file_plugin("context-menu.ts").is_builtin_deno());
    }

    /// A typo is much harder to diagnose if the error names only one of the places looked in.
    #[test]
    fn a_missing_script_plugin_names_every_path_tried() {
        let plugin_dir = tempdir().unwrap();
        let error = file_plugin("nope.ts")
            .resolve_source(Some(plugin_dir.path()))
            .unwrap_err()
            .to_string();

        assert!(error.contains("nope.ts"), "{error}");
        assert!(
            error.contains(&plugin_dir.path().display().to_string()),
            "the folder that was searched should be named too: {error}"
        );
    }

    #[test]
    fn a_script_plugin_resolves_with_no_user_plugin_dir_at_all() {
        let elsewhere = tempdir().unwrap();
        let explicit = elsewhere.path().join("keys.ts");
        fs::write(&explicit, "-- standalone").unwrap();

        assert_eq!(
            file_plugin(&explicit.display().to_string())
                .resolve_source(None)
                .unwrap(),
            (explicit, "-- standalone".to_owned())
        );
    }

    #[test]
    fn an_ordinary_builtin_is_still_wasm() {
        assert_eq!(
            PluginConfig::builtin_lookup_example().runtime(),
            super::PluginRuntime::Wasm
        );
    }

    #[test]
    fn configured_builtin_plugin_directory_takes_precedence() {
        let data_dir = tempdir().unwrap();
        let builtin_plugins_dir = tempdir().unwrap();
        let plugin_path = builtin_plugins_dir.path().join("strider.wasm");
        fs::write(&plugin_path, b"override").unwrap();

        let plugin = builtin_plugin("strider");
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
        let plugin = builtin_plugin("strider");

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
        fs::write(plugin_dir.join("strider.wasm"), b"from-data-dir").unwrap();

        let plugin = builtin_plugin("strider");
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
        let installed = yinz_runtime::home::builtin_plugin_dir(data_dir.path());
        fs::create_dir_all(&installed).unwrap();
        fs::write(installed.join("strider.wasm"), b"installed").unwrap();

        // A release build has no debug dir to shadow the install; a debug build (this test)
        // does, which is the point of the debug dir - so tell them apart by whether the
        // checkout's debug wasm exists.
        #[cfg(debug_assertions)]
        let debug_copy = std::path::Path::new(super::BUILTIN_PLUGIN_DEBUG_DIR).join("strider.wasm");
        #[cfg(not(debug_assertions))]
        let debug_copy = PathBuf::from("/nonexistent/strider.wasm");
        let bytes = builtin_plugin("strider")
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
        fs::write(plugin_dir.join("strider.wasm"), b"from-data-dir").unwrap();
        let installed = yinz_runtime::home::builtin_plugin_dir(data_dir.path());
        fs::create_dir_all(&installed).unwrap();
        fs::write(installed.join("strider.wasm"), b"installed").unwrap();

        assert_eq!(
            builtin_plugin("strider")
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
        fs::write(other_version.join("strider.wasm"), b"stale").unwrap();

        let bytes = builtin_plugin("strider")
            .resolve_wasm_bytes(data_dir.path(), None)
            .ok();
        assert_ne!(bytes.as_deref(), Some(&b"stale"[..]));
    }

    /// The documented order, which `yinz setup --check` prints verbatim.
    #[test]
    fn the_lookup_order_is_stable() {
        let data_dir = std::path::Path::new("/data");
        let configured = std::path::Path::new("/configured");
        let paths = builtin_plugin("strider").wasm_lookup_paths(data_dir, Some(configured));

        let mut expected = vec![
            PathBuf::from("/configured/strider.wasm"),
            PathBuf::from("strider"),
            PathBuf::from("strider.wasm"),
            PathBuf::from("/data/plugins/strider.wasm"),
        ];
        #[cfg(debug_assertions)]
        expected.push(std::path::Path::new(super::BUILTIN_PLUGIN_DEBUG_DIR).join("strider.wasm"));
        expected.push(yinz_runtime::home::builtin_plugin_dir(data_dir).join("strider.wasm"));
        expected.push(yinz_runtime::home::system_builtin_plugin_dir().join("strider.wasm"));
        expected.push(
            yinz_runtime::home::system_data_dir()
                .join("plugins")
                .join("strider.wasm"),
        );
        expected.push(std::path::Path::new(super::BUILTIN_PLUGIN_RELEASE_DIR).join("strider.wasm"));
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
        let plugin = builtin_plugin("strider");

        assert!(!plugin
            .resolve_wasm_bytes(data_dir.path(), None)
            .unwrap()
            .is_empty());
    }
}
