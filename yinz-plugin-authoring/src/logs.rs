//! Source-aware log selection, shared by the CLI and plugin host.
use std::path::{Path, PathBuf};
use yinz_config::plugins::PluginConfig;
use yinz_runtime::errors::prelude::*;
use yinz_types::input::{layout::RunPluginLocation, plugins::PluginAliases};

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct LogIdentity {
    path: Option<String>,
    repository: Option<String>,
    builtin: Option<String>,
}

use yinz_runtime::common_path::source_path;

// Preserve non-UTF-8 filesystem names as well as control characters without lossy collisions.
fn path_identity(path: &Path) -> String {
    path.as_os_str()
        .as_encoded_bytes()
        .escape_ascii()
        .to_string()
}

impl LogIdentity {
    pub fn new(location: &RunPluginLocation, path: &Path, cwd: &Path) -> Self {
        match location {
            RunPluginLocation::Yinz(tag) => Self {
                builtin: Some(tag.to_string()),
                ..Self::default()
            },
            RunPluginLocation::File(_) => Self {
                path: Some(path_identity(&source_path(path, cwd))),
                ..Self::default()
            },
            RunPluginLocation::Git(repository) => Self {
                path: Some(path_identity(&source_path(path, cwd))),
                ..Self::repository(repository)
            },
        }
    }

    /// Repository failures before installation have no resolved source file.
    pub fn repository(repository: &yinz_types::input::git_plugin::GitPlugin) -> Self {
        Self {
            repository: Some(repository.descriptor()),
            ..Self::default()
        }
    }

    pub fn marker(&self) -> String {
        format!(
            "[plugin-source:{}]",
            serde_json::to_string(self).expect("log identity is serializable")
        )
    }
}

#[derive(Debug)]
pub enum LogTarget {
    File(PathBuf),
    Repository(String),
    Builtin(String),
    Legacy(PathBuf),
}

/// Resolve explicit filesystem targets without canonicalizing or requiring a source file.
/// Bare names remain the caller's managed-plugin lookup.
pub fn resolve_explicit_plugin_path(target: &str, cwd: &Path) -> Result<Option<PathBuf>> {
    if target.starts_with("file:") {
        let location =
            yinz_runtime::plugin_location::parse_plugin_location(target, Some(cwd.to_owned()))
                .context("resolve plugin URL")?;
        if let RunPluginLocation::File(path) = location {
            return Ok(Some(path));
        }
    }
    if Path::new(target).is_absolute() || target.starts_with("./") || target.starts_with("../") {
        return Ok(Some(cwd.join(target)));
    }
    Ok(None)
}

pub fn resolve_log_target(
    name: &str,
    dir: Option<&Path>,
    aliases: &PluginAliases,
    cwd: &Path,
) -> Result<LogTarget> {
    if let Some(plugin) = aliases.aliases.get(name) {
        return Ok(match &plugin.location {
            RunPluginLocation::File(path) => {
                let config = PluginConfig::from_run_plugin(plugin)
                    .context("resolve file alias configuration")?;
                let resolved = config.resolve_source(dir).map(|(path, _)| path);
                // Keep diagnostics accessible when the source can no longer be read.
                LogTarget::File(source_path(resolved.as_deref().unwrap_or(path), cwd))
            },
            RunPluginLocation::Git(repository) => LogTarget::Repository(repository.descriptor()),
            RunPluginLocation::Yinz(tag) => LogTarget::Builtin(tag.to_string()),
        });
    }
    if let Some(path) = resolve_explicit_plugin_path(name, cwd)? {
        return Ok(LogTarget::File(source_path(&path, cwd)));
    }
    if name.contains(':') {
        let location =
            yinz_runtime::plugin_location::parse_plugin_location(name, Some(cwd.to_owned()))
                .context("resolve plugin log URL")?;
        return Ok(match location {
            RunPluginLocation::File(path) => LogTarget::File(source_path(&path, cwd)),
            RunPluginLocation::Git(repository) => LogTarget::Repository(repository.descriptor()),
            RunPluginLocation::Yinz(tag) => LogTarget::Builtin(tag.to_string()),
        });
    }
    let dir = dir.ok_or_else(|| {
        anyhow!("no plugin folder is configured; set `user_plugin_dir` in the Yinz config")
    })?;
    Ok(LogTarget::Legacy(
        yinz_plugin_api::authoring::resolve_plugin_file(dir, name).map_err(anyhow::Error::msg)?,
    ))
}

pub fn log_target_tail(log_file: &Path, target: &LogTarget, lines: usize) -> Result<String> {
    let log = match std::fs::read_to_string(log_file) {
        Ok(log) => log,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            anyhow::bail!("no yinz log at {}", log_file.display())
        },
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", log_file.display()))
        },
    };
    let matching: Vec<_> = log
        .lines()
        .filter(|line| {
            let Some((_, json)) = line.split_once("[plugin-source:") else {
                return match target {
                    LogTarget::Legacy(path) => {
                        path.file_name()
                            .is_some_and(|name| line.contains(&*name.to_string_lossy()))
                            || path
                                .file_stem()
                                .and_then(|stem| stem.to_str())
                                .is_some_and(|stem| {
                                    yinz_runtime::consts::BUILTIN_PLUGIN_NAMES.contains(&stem)
                                        && line.contains(&format!("[steel:{stem}]"))
                                })
                    },
                    _ => false,
                };
            };
            let mut stream = serde_json::Deserializer::from_str(json).into_iter::<LogIdentity>();
            let Some(Ok(identity)) = stream.next() else {
                return false;
            };
            if !json[stream.byte_offset()..].starts_with(']') {
                return false;
            }
            match target {
                LogTarget::File(path) => {
                    identity.path.as_deref() == Some(path_identity(path).as_str())
                },
                LogTarget::Repository(repository) => {
                    identity.repository.as_ref() == Some(repository)
                },
                LogTarget::Builtin(tag) => identity.builtin.as_ref() == Some(tag),
                LogTarget::Legacy(path) => {
                    identity.path.as_ref().is_some_and(|source| {
                        source.rsplit('/').next()
                            == path
                                .file_name()
                                .map(|name| name.as_encoded_bytes().escape_ascii().to_string())
                                .as_deref()
                    }) || identity.builtin.as_deref().is_some_and(|tag| {
                        path.file_stem().and_then(|stem| stem.to_str()) == Some(tag)
                    })
                },
            }
        })
        .collect();
    let mut output = matching[matching.len().saturating_sub(lines.max(1))..].join("\n");
    if !output.is_empty() {
        output.push('\n');
    }
    Ok(output)
}

#[cfg(test)]
#[path = "unit/logs.rs"]
mod tests;
