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
    match crate::parse_plugin_location_with_relative(url, cwd) {
        Ok((location, relative_path)) => Ok(RunPluginOrAlias::RunPlugin(RunPlugin {
            _allow_exec_host_cmd: false,
            location,
            configuration: configuration
                .as_ref()
                .map(|c| PluginUserConfiguration::new(c.clone()))
                .unwrap_or_default(),
            relative_path,
            ..Default::default()
        })),
        Err(PluginsConfigError::InvalidUrlScheme(_)) | Err(PluginsConfigError::InvalidUrl(..)) => {
            let mut plugin_alias = PluginAlias::new(url, configuration, None);
            if let Some(alias_dict) = alias_dict {
                plugin_alias.run_plugin = alias_dict
                    .aliases
                    .get(url)
                    .map(|r| r.clone().merge_configuration(configuration));
            }
            Ok(RunPluginOrAlias::Alias(plugin_alias))
        },
        Err(e) => Err(format!("Failed to parse plugin location {url}: {}", e)),
    }
}
pub fn run_plugin_from_url(url: &str) -> Result<RunPlugin, PluginsConfigError> {
    let location = crate::parse_plugin_location(url, None)?;
    Ok(RunPlugin {
        location,
        ..Default::default()
    })
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
        "yinz" => Ok((RunPluginLocation::Yinz(PluginTag::new(decoded_path)), None)),
        "file" => {
            // Set by the one branch that joins a cwd, so it is exactly "what the user wrote,
            // before it was made absolute".
            let mut relative = None;
            let path = if location.starts_with("file:/") {
                // Path is absolute, its safe to use URL path.
                //
                // This is the case if the scheme and : delimiter are followed by a / slash
                PathBuf::from(decoded_path.as_str())
            } else if location.starts_with("file:~") {
                // Unwrap is safe here since location is a valid URL
                PathBuf::from(location.strip_prefix("file:").unwrap())
            } else {
                // URL dep doesn't handle relative paths with `file` schema properly,
                // it always makes them absolute. Use raw location string instead.
                //
                // Unwrap is safe here since location is a valid URL
                let stripped = location.strip_prefix("file:").unwrap();
                relative = Some(expand_plugin_path(PathBuf::from(stripped)));
                match cwd {
                    Some(cwd) => cwd.join(stripped),
                    None => PathBuf::from(stripped),
                }
            };
            Ok((RunPluginLocation::File(expand_plugin_path(path)), relative))
        },
        // Plugins are never downloaded. Refuse remote urls here, everywhere a plugin url is
        // read (config, layouts, hooks, the CLI and a plugin loading another), rather than
        // letting them fall through to an alias lookup that names nothing.
        "https" | "http" => Err(PluginsConfigError::RemotePluginUnsupported(url)),
        _ => Err(PluginsConfigError::InvalidUrlScheme(url)),
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
