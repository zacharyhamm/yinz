//! Provider metadata is evaluated only in the supervised compilation subprocess.
use std::path::Path;
use yinz_runtime::errors::prelude::*;
use yinz_types::agent::provider::AgentProvider;

pub(crate) fn inspect(path: &Path, source: &str) -> Result<AgentProvider> {
    let mut runtime = yinz_steel::Runtime::new(|_| {
        bail!("host commands are unavailable during provider inspection")
    })?;
    let mut root = super::source_root(path, None)?;
    runtime
        .eval_with_source_root(source, &mut root)
        .with_context(|| format!("evaluate Scheme provider {}", path.display()))?;
    let provider: AgentProvider = serde_json::from_value(runtime.value("agent-provider")?)
        .context("plugin must define an agent-provider descriptor")?;
    provider.validate().map_err(|error| anyhow!(error))?;
    Ok(provider)
}

#[cfg(test)]
#[path = "unit/agent_provider.rs"]
mod tests;
