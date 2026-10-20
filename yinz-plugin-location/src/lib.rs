//! Decode plugin locations using host path expansion.
use std::{collections::BTreeMap, path::PathBuf};
use url::Url;
use yinz_types::input::{
    layout::{
        PluginAlias, PluginUserConfiguration, RunPlugin, RunPluginLocation, RunPluginOrAlias,
    },
    plugins::{PluginAliases, PluginTag, PluginsConfigError},
};
pub fn run_plugin_or_alias_from_url(
    url: &str,
    configuration: &Option<BTreeMap<String, String>>,
    alias_dict: Option<&PluginAliases>,
    cwd: Option<PathBuf>,
) -> Result<RunPluginOrAlias, String> {
    let mut plugin =
        match run_plugin_from_url_with_cwd(url, cwd) {
            Ok(plugin) => Ok(RunPluginOrAlias::RunPlugin(RunPlugin {
                configuration: configuration
                    .as_ref()
                    .map(|c| PluginUserConfiguration::new(c.clone()))
                    .unwrap_or_default(),
                ..plugin
            })),
            Err(PluginsConfigError::InvalidUrlScheme(_))
            | Err(PluginsConfigError::InvalidUrl(..)) => Ok(RunPluginOrAlias::Alias(
                PluginAlias::new(url, configuration, None),
            )),
            Err(e) => Err(format!("Failed to parse plugin location {url}: {}", e)),
        }?;
    plugin.populate_run_plugin_if_needed(alias_dict.unwrap_or(&PluginAliases::default()));
    Ok(plugin)
}
pub fn run_plugin_from_url(url: &str) -> Result<RunPlugin, PluginsConfigError> {
    run_plugin_from_url_with_cwd(url, None)
}
pub fn parse_plugin_location(
    location: &str,
    cwd: Option<PathBuf>,
) -> Result<RunPluginLocation, PluginsConfigError> {
    crate::parse_plugin_location_with_relative(location, cwd).map(|(location, _)| location)
}
pub fn parse_plugin_location_with_relative(
    location: &str,
    cwd: Option<PathBuf>,
) -> Result<(RunPluginLocation, Option<PathBuf>), PluginsConfigError> {
    run_plugin_from_url_with_cwd(location, cwd)
        .map(|plugin| (plugin.location, plugin.relative_path))
}

pub fn run_plugin_from_url_with_cwd(
    location: &str,
    cwd: Option<PathBuf>,
) -> Result<RunPlugin, PluginsConfigError> {
    let url = Url::parse(location);
    let path = url.as_ref().map(|url| url.path()).unwrap_or(location);
    let decoded_path = percent_encoding::percent_decode_str(path)
        .decode_utf8_lossy()
        .into_owned();

    if std::path::Path::new(decoded_path.as_str())
        .extension()
        .and_then(|ext| ext.to_str())
        == Some("lua")
    {
        return Err(PluginsConfigError::UnsupportedLuaPlugin(
            location.to_owned(),
        ));
    }
    let url = url?;
    match url.scheme() {
        "git" => Ok(RunPlugin {
            location: RunPluginLocation::Git(
                yinz_types::input::git_plugin::GitPlugin::from_descriptor(location)
                    .map_err(|e| PluginsConfigError::InvalidRepository(e.into()))?,
            ),
            ..Default::default()
        }),
        "yinz" => Ok(RunPlugin {
            location: RunPluginLocation::Yinz(PluginTag::new(decoded_path)),
            ..Default::default()
        }),
        "file" => {
            // Preserve the relative spelling for the user-plugin-directory fallback.
            let (_, raw_path) = location.split_once(':').expect("a valid URL has a scheme");
            let is_relative = !raw_path.starts_with(['/', '~']);
            let path = if raw_path.starts_with('/') {
                PathBuf::from(decoded_path.as_str())
            } else {
                // Relative file locations are literal paths, including percent signs.
                PathBuf::from(raw_path)
            };
            let path = expand_plugin_path(path);
            let relative = is_relative.then(|| path.clone());
            let path = cwd.map_or_else(|| path.clone(), |cwd| cwd.join(&path));
            // Wasm plugins parse locations too, without access to the host filesystem.
            #[cfg(not(target_arch = "wasm32"))]
            let path = if path.is_relative() {
                std::env::current_dir()
                    .map_err(|_| PluginsConfigError::InvalidPluginLocation(path.clone()))?
                    .join(path)
            } else {
                path
            };
            let declared_path = Some(path.clone());
            #[cfg(not(target_arch = "wasm32"))]
            let path = path.canonicalize().unwrap_or(path);
            Ok(RunPlugin {
                location: RunPluginLocation::File(path),
                relative_path: relative,
                declared_path,
                ..Default::default()
            })
        },
        // Plugins are never downloaded. Refuse remote urls here, everywhere a plugin url is
        // read (config, layouts, hooks, the CLI and a plugin loading another), rather than
        // letting them fall through to an alias lookup that names nothing.
        "https" | "http" => Err(PluginsConfigError::RemotePluginUnsupported(url)),
        _ => Err(PluginsConfigError::InvalidUrlScheme(url)),
    }
}
/// Refresh an incoming launch request without changing identities of running instances.
#[cfg(not(target_arch = "wasm32"))]
pub fn refresh_file_identity(request: &mut RunPluginOrAlias) {
    let plugin = match request {
        RunPluginOrAlias::RunPlugin(plugin) => Some(plugin),
        RunPluginOrAlias::Alias(alias) => alias.run_plugin.as_mut(),
    };
    if let Some(plugin) = plugin
        && let RunPluginLocation::File(location) = &mut plugin.location
    {
        let declared = plugin.declared_path.as_ref().unwrap_or(location);
        *location = declared.canonicalize().unwrap_or_else(|_| declared.clone());
    }
}

fn expand_plugin_path(path: PathBuf) -> PathBuf {
    match shellexpand::full(&path.to_string_lossy()) {
        Ok(expanded) => PathBuf::from(expanded.as_ref()),
        Err(e) => {
            log::error!("Failed to shell expand plugin path: {}", e);
            path
        },
    }
}

#[cfg(test)]
mod remote_plugin_tests {
    use super::*;

    #[test]
    fn remote_plugin_urls_are_refused() {
        for url in [
            "https://example.com/plugin.wasm",
            "http://example.com/plugin",
            "https://example.com/plugin.ts?v=1",
        ] {
            let error = parse_plugin_location_with_relative(url, None).unwrap_err();
            assert!(
                matches!(error, PluginsConfigError::RemotePluginUnsupported(_)),
                "{url}: {error:?}"
            );
            let message = error.to_string();
            assert!(
                message.contains("remote plugins are not supported") && message.contains(url),
                "{message}"
            );
            // Not mistaken for an alias named after the url.
            assert!(run_plugin_or_alias_from_url(url, &None, None, None).is_err());
        }
    }
}

#[cfg(test)]
mod retired_script_tests {
    use super::*;
    #[test]
    fn lua_urls_fail_before_aliases() {
        for url in [
            "old.lua",
            "/tmp/old.lua",
            "file:old.lua",
            "file:/tmp/old.lua",
            "https://example.com/old.lua",
            "https://example.com/old%2Elua",
        ] {
            assert!(parse_plugin_location(url, None)
                .unwrap_err()
                .to_string()
                .contains("Lua plugins are no longer supported"));
            assert!(run_plugin_or_alias_from_url(url, &None, None, None).is_err());
        }
    }
}

/// Resolve a declaration's local repository against the active configuration directory.
pub fn resolve_repository_path(
    plugin: &mut yinz_types::input::git_plugin::GitPlugin,
    base: &std::path::Path,
) -> Result<(), PluginsConfigError> {
    if plugin.repo.is_empty() || plugin.repo == "file:" {
        return Err(PluginsConfigError::InvalidRepository(
            "repo must not be empty".into(),
        ));
    }
    if plugin.repo.starts_with("file:") {
        if Url::parse(&plugin.repo)?
            .host_str()
            .is_some_and(|host| host != "localhost")
        {
            return Err(PluginsConfigError::InvalidRepository(
                "file URLs must refer to the local machine".into(),
            ));
        }
        let parsed = run_plugin_from_url_with_cwd(&plugin.repo, Some(base.to_owned()))?;
        if let Some(path) = parsed.declared_path {
            // Parsing validates before the active config directory is known.
            let path = if base.as_os_str().is_empty() {
                parsed.relative_path.unwrap_or(path)
            } else {
                path
            };
            plugin.repo = path.to_string_lossy().into_owned();
        }
    } else if plugin.is_local() {
        let path = expand_plugin_path(PathBuf::from(&plugin.repo));
        plugin.repo = base.join(path).to_string_lossy().into_owned();
    }
    plugin
        .validate()
        .map_err(|e| PluginsConfigError::InvalidRepository(e.into()))
}

#[cfg(test)]
#[path = "unit/git_plugins.rs"]
mod git_plugins;

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "unit/file_identity.rs"]
mod file_identity;
