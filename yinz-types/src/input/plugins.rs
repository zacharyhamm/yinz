use super::layout::{RunPlugin, RunPluginLocation, RunPluginOrAlias};
pub use crate::data::PluginTag;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Presentation policy for a resolved plugin, independent of its launch alias.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PluginPresentation {
    pub tab_bar: bool,
    pub opening_ripple: bool,
}

impl RunPluginOrAlias {
    pub fn presentation(&self) -> PluginPresentation {
        let plugin = match self {
            Self::RunPlugin(plugin) => Some(plugin),
            Self::Alias(alias) => alias.run_plugin.as_ref(),
        };
        let Some(RunPlugin {
            location: RunPluginLocation::Yinz(tag),
            ..
        }) = plugin
        else {
            return PluginPresentation::default();
        };
        match tag.as_str() {
            "tab-bar" => PluginPresentation {
                tab_bar: true,
                ..Default::default()
            },
            "agents" => PluginPresentation {
                opening_ripple: true,
                ..Default::default()
            },
            _ => PluginPresentation::default(),
        }
    }
}

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

use std::path::PathBuf;
use url::Url;
#[derive(thiserror::Error, Debug, PartialEq)]
pub enum PluginsConfigError {
    #[error("Duplication in plugin tag names is not allowed: '{}'", String::from(.0.clone()))]
    DuplicatePlugins(PluginTag),
    #[error("Invalid repository plugin: {0}")]
    InvalidRepository(String),
    #[error("Failed to parse url: {0:?}")]
    InvalidUrl(#[from] url::ParseError),
    #[error("Only 'file:', 'yinz:' and 'git://plugin' locations are supported for plugin lookup. '{0}' does not match any of them.")]
    InvalidUrlScheme(Url),
    #[error("Could not find plugin at the path: '{0:?}'")]
    InvalidPluginLocation(PathBuf),
    #[error("remote plugins are not supported ('{0}'): load a local plugin with file:<path>")]
    RemotePluginUnsupported(Url),
    #[error("Lua plugins are no longer supported ({0}); migrate to Steel Scheme")]
    UnsupportedLuaPlugin(String),
}

#[cfg(test)]
#[path = "unit/plugins.rs"]
mod tests;
