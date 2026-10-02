//! Offline provider discovery executes TypeScript only in the compilation subprocess.
use super::{limits, loader::PluginModuleLoader, transport, HostState, SDK};
use deno_core::{v8, RuntimeOptions};
use std::{path::Path, rc::Rc};
use yinz_runtime::errors::prelude::*;
use yinz_types::agent::provider::AgentProvider;

/// Read metadata from the same TypeScript entrypoint used by the live plugin host.
pub fn inspect_agent_provider(location: &str) -> Result<AgentProvider> {
    let (path, source) = if let Some(tag) = location.strip_prefix("yinz:") {
        let source = yinz_deno_builtins::builtin_deno_plugin(tag)
            .context("unknown builtin agent provider")?;
        (
            Path::new(super::loader::EMBEDDED_ROOT).join(format!("{tag}.ts")),
            source.to_owned(),
        )
    } else {
        let path = Path::new(location)
            .canonicalize()
            .context("resolve agent plugin")?;
        if !matches!(
            path.extension().and_then(|extension| extension.to_str()),
            Some("ts" | "mts")
        ) {
            bail!("agent plugins must be TypeScript (.ts or .mts)");
        }
        let source = yinz_plugin_authoring::limits::read_source(&path)?;
        (path, source)
    };
    yinz_plugin_authoring::worker::agent_provider(&path, &source)
}

pub(super) fn inspect(path: &Path, source: &str) -> Result<AgentProvider> {
    let loader = Rc::new(
        if let Ok(relative) = path.strip_prefix(super::loader::EMBEDDED_ROOT) {
            PluginModuleLoader::embedded(
                relative
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .context("provider builtin name")?,
                source,
            )?
        } else {
            PluginModuleLoader::new(path, source)?
        },
    );
    let limits = limits::Limits::new(path.display().to_string());
    let mut runtime = limits.runtime(RuntimeOptions {
        module_loader: Some(loader.clone()),
        extensions: vec![super::yinz_host::init()],
        ..Default::default()
    })?;
    runtime.op_state().borrow_mut().put(HostState {
        env: None,
        log_tag: path.display().to_string(),
    });
    runtime
        .execute_script("yinz:sdk-bootstrap", SDK)
        .context("initialize provider SDK")?;
    let event_loop = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("create provider event loop")?;
    event_loop.block_on(async {
        let module = runtime
            .load_main_es_module(loader.entrypoint())
            .await
            .context("load TypeScript agent provider")?;
        let evaluated = runtime.mod_evaluate(module);
        runtime
            .run_event_loop(Default::default())
            .await
            .context("evaluate provider modules")?;
        evaluated
            .await
            .context("evaluate TypeScript agent provider")?;
        limits.check()?;
        let namespace = runtime
            .get_module_namespace(module)
            .context("read provider exports")?;
        deno_core::scope!(scope, runtime);
        let namespace = v8::Local::new(scope, namespace);
        let key =
            v8::String::new(scope, "agentProvider").context("allocate provider export name")?;
        let value = namespace
            .get(scope, key.into())
            .context("read agentProvider export")?;
        let provider: AgentProvider = serde_json::from_value(transport::from_v8(scope, value)?)
            .context("plugin must export an agentProvider descriptor")?;
        provider.validate().map_err(|error| anyhow!(error))?;
        Ok(provider)
    })
}

#[cfg(test)]
#[path = "unit/agent_provider.rs"]
mod tests;
