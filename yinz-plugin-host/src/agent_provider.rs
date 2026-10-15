//! Offline provider discovery in a supervised compilation subprocess.
use std::path::Path;
use yinz_runtime::errors::prelude::*;
use yinz_types::agent::provider::AgentProvider;
/// Read metadata from the same source entrypoint used by the live plugin host.
pub fn inspect_agent_provider(location: &str) -> Result<AgentProvider> {
    let (path, source) = if let Some(tag) = location.strip_prefix("yinz:") {
        let source = yinz_steel_builtins::builtin_steel_plugin(tag)
            .context("unknown builtin agent provider")?;
        (
            Path::new(crate::steel::EMBEDDED_ROOT).join(format!("{tag}.scm")),
            source.to_owned(),
        )
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
