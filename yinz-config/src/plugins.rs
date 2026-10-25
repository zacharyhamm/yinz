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
#[path = "unit/plugins_tests.rs"]
mod tests;
