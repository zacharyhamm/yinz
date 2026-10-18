//! Offline provider discovery in a supervised compilation subprocess.
use std::path::Path;
use yinz_runtime::errors::prelude::*;
use yinz_types::agent::provider::AgentProvider;
/// Read metadata from the same source entrypoint used by the live plugin host.
pub fn inspect_agent_provider(location: &str, data_dir: &Path) -> Result<AgentProvider> {
    let (path, source) = if let Some(tag) = location.strip_prefix("yinz:") {
        let path = yinz_steel_builtins::builtin_path(data_dir, tag)?;
        let source = yinz_steel::modules::SourceRoot::file(&path, None)?.read_entry()?;
        (path, source)
    } else {
        let path = Path::new(location)
            .canonicalize()
            .context("resolve agent plugin")?;
        if !matches!(
            path.extension().and_then(|extension| extension.to_str()),
            Some("scm" | "ss" | "sld")
        ) {
            bail!("agent plugins must be Steel Scheme (.scm, .ss or .sld)");
        }
        let source = yinz_plugin_authoring::limits::read_source(&path)?;
        (path, source)
    };
    yinz_plugin_authoring::worker::agent_provider(&path, &source)
}

#[cfg(test)]
#[path = "unit/agent_provider.rs"]
mod tests;
