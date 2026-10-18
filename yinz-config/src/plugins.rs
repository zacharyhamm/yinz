//! Plugins configuration metadata
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use yinz_runtime::consts::{BUILTIN_PLUGIN_NAMES, MAX_PLUGIN_SOURCE_BYTES};
use yinz_runtime::errors::prelude::*;
use {
    yinz_types::input::layout::PluginUserConfiguration, yinz_types::input::layout::RunPlugin,
    yinz_types::input::layout::RunPluginLocation,
};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub struct PluginConfig {
    /// Path of the plugin, see resolve_source for lookup semantics
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
            .take(MAX_PLUGIN_SOURCE_BYTES as u64 + 1)
            .read_to_string(&mut source)?;
        if source.len() > MAX_PLUGIN_SOURCE_BYTES {
            return Err(std::io::Error::other(
                "plugin exceeds 8 MiB source-file budget",
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
    /// configuration block; builtin pane drivers always share their session lifetime.
    pub fn is_shared(&self) -> bool {
        // Pane drivers must survive detach and create only one pane per launch.
        if matches!(&self.location, RunPluginLocation::Yinz(tag) if matches!(tag.to_string().as_str(), "new-agent" | "agent-opencode" | "file-finder"))
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
        let configured = match &run_plugin.location {
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
                if BUILTIN_PLUGIN_NAMES.contains(&tag.as_str()) {
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
        };
        configured.filter(|plugin| {
            plugin.is_builtin()
                || matches!(
                    plugin
                        .path
                        .extension()
                        .and_then(|extension| extension.to_str()),
                    Some("scm" | "ss" | "sld")
                )
        })
    }
    /// Read Scheme source, trying the declared path first and the user plugin directory only
    /// when a relative declaration is missing. Explicit absolute paths never fall back.
    /// Return the selected path so reload and diagnostics retain the same source identity.
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
        if self.is_builtin() {
            return Err(anyhow!(
                "'{}' is an installed builtin; the plugin host resolves it in the selected data directory",
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
}

#[cfg(test)]
mod tests {
    use super::{PluginConfig, RunPlugin, RunPluginLocation};
    use std::fs;
    use tempfile::tempdir;
    use yinz_types::data::PluginTag;

    #[test]
    fn steel_sources_resolve_by_extension_and_user_plugin_directory() {
        let plugins = tempdir().unwrap();
        let cwd = tempdir().unwrap();
        for extension in ["scm", "ss", "sld"] {
            let name = format!("keys.{extension}");
            let source = "(define (update event) #f)";
            fs::write(plugins.path().join(&name), source).unwrap();
            let run = yinz_runtime::plugin_location::run_plugin_from_url_with_cwd(
                &format!("file:{name}"),
                Some(cwd.path().to_owned()),
            )
            .unwrap();
            let plugin = PluginConfig::from_run_plugin(&run).unwrap();
            assert_eq!(
                plugin.resolve_source(Some(plugins.path())).unwrap(),
                (plugins.path().join(&name), source.to_owned())
            );
            let explicit = cwd.path().join(&name);
            assert!(file_plugin(explicit.to_str().unwrap())
                .resolve_source(Some(plugins.path()))
                .is_err());
            fs::write(&explicit, "(define explicit #t)").unwrap();
            assert_eq!(
                plugin.resolve_source(Some(plugins.path())).unwrap().1,
                "(define explicit #t)"
            );
        }
    }

    #[test]
    fn unsupported_source_extensions_are_rejected() {
        for name in [
            "keys",
            "keys.wasm",
            "keys.js",
            "keys.ts",
            "keys.mjs",
            "keys.mts",
            "keys.SCM",
        ] {
            assert!(
                PluginConfig::from_run_plugin(&RunPlugin {
                    location: RunPluginLocation::File(name.into()),
                    ..Default::default()
                })
                .is_none(),
                "{name}"
            );
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

    #[test]
    fn file_finder_has_one_driver_per_launch_even_when_sharing_is_disabled() {
        let mut plugin = builtin_plugin("file-finder");
        assert!(plugin.is_shared());
        plugin
            .initial_userspace_configuration
            .insert("shared", "false");
        assert!(plugin.is_shared());
        assert!(!file_plugin("file-finder.scm").is_shared());
    }

    fn file_plugin(path: &str) -> PluginConfig {
        PluginConfig::from_run_plugin(&RunPlugin {
            location: RunPluginLocation::File(std::path::PathBuf::from(path)),
            ..Default::default()
        })
        .unwrap()
    }

    #[test]
    fn a_script_plugin_is_found_in_the_user_plugin_dir_by_name() {
        let plugin_dir = tempdir().unwrap();
        fs::write(plugin_dir.path().join("keys.scm"), "-- from the dir").unwrap();

        assert_eq!(
            file_plugin("keys.scm")
                .resolve_source(Some(plugin_dir.path()))
                .unwrap(),
            (
                plugin_dir.path().join("keys.scm"),
                "-- from the dir".to_owned()
            )
        );
    }

    /// A plugin as the `yinz plugin -- file:keys.scm` path builds one: the URL was relative, so
    /// the cwd has already been joined onto it and the path is absolute by now. Only
    /// `relative_path` still knows the plugin was named `keys.scm`, and without it the folder
    /// would be searched for `$cwd/keys.scm` - which `Path::join` returns unchanged, so nothing
    /// in the folder could ever match.
    #[test]
    fn a_relative_url_launched_from_elsewhere_is_found_in_the_user_plugin_dir() {
        let plugin_dir = tempdir().unwrap();
        fs::write(plugin_dir.path().join("keys.scm"), "-- from the dir").unwrap();
        let cwd = tempdir().unwrap();

        let (location, relative_path) =
            yinz_runtime::plugin_location::parse_plugin_location_with_relative(
                "file:keys.scm",
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
                plugin_dir.path().join("keys.scm"),
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
        fs::write(plugin_dir.path().join("keys.scm"), "-- from the dir").unwrap();
        let elsewhere = tempdir().unwrap();
        let missing = elsewhere.path().join("keys.scm");

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
        assert!(error.contains("keys.scm"), "{error}");
    }

    /// The path as given always wins, so an existing declaration cannot start meaning a
    /// different file the day a same-named plugin appears in the folder.
    #[test]
    fn a_path_that_exists_beats_the_user_plugin_dir() {
        let plugin_dir = tempdir().unwrap();
        fs::write(plugin_dir.path().join("keys.scm"), "-- from the dir").unwrap();
        let elsewhere = tempdir().unwrap();
        let explicit = elsewhere.path().join("keys.scm");
        fs::write(&explicit, "-- from the explicit path").unwrap();

        assert_eq!(
            file_plugin(&explicit.display().to_string())
                .resolve_source(Some(plugin_dir.path()))
                .unwrap(),
            (explicit, "-- from the explicit path".to_owned())
        );
    }

    /// The plugin host serves a Scheme builtin's compiled-in source before it would ever ask
    /// for a file, so a request that reaches here anyway must fail rather than find a same-named
    /// file posing as the builtin.
    #[test]
    fn a_builtin_tag_is_never_read_from_the_user_plugin_directory() {
        let plugin_dir = tempdir().unwrap();
        for name in yinz_runtime::consts::BUILTIN_PLUGIN_NAMES {
            fs::write(plugin_dir.path().join(name), "-- impostor").unwrap();
            fs::write(plugin_dir.path().join(format!("{name}.scm")), "-- impostor").unwrap();
            let plugin = builtin_plugin(name);
            assert!(plugin.is_builtin());
            let error = plugin
                .resolve_source(Some(plugin_dir.path()))
                .unwrap_err()
                .to_string();
            assert!(error.contains("installed builtin"), "{error}");
        }
        assert!(!file_plugin("context-menu.scm").is_builtin());
    }

    /// A typo is much harder to diagnose if the error names only one of the places looked in.
    #[test]
    fn a_missing_script_plugin_names_every_path_tried() {
        let plugin_dir = tempdir().unwrap();
        let error = file_plugin("nope.scm")
            .resolve_source(Some(plugin_dir.path()))
            .unwrap_err()
            .to_string();

        assert!(error.contains("nope.scm"), "{error}");
        assert!(
            error.contains(&plugin_dir.path().display().to_string()),
            "the folder that was searched should be named too: {error}"
        );
    }

    #[test]
    fn a_script_plugin_resolves_with_no_user_plugin_dir_at_all() {
        let elsewhere = tempdir().unwrap();
        let explicit = elsewhere.path().join("keys.scm");
        fs::write(&explicit, "-- standalone").unwrap();

        assert_eq!(
            file_plugin(&explicit.display().to_string())
                .resolve_source(None)
                .unwrap(),
            (explicit, "-- standalone".to_owned())
        );
    }
}
