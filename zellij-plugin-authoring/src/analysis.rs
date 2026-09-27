//! The client side of plugin analysis. The SWC visitor that finds grants, lint findings and
//! the outline runs in the compilation worker (`zellij_plugin_host`'s `deno::analysis`).
use std::path::Path;
use zellij_types::authoring::{PluginCheckMode, PluginCheckReport};

/// Syntax checks parse in one worker; the other modes also type-check and run the strict
/// authoring tools. A worker that fails is reported as a diagnostic.
pub fn check(path: &Path, source: &str, mode: PluginCheckMode) -> PluginCheckReport {
    let result = if mode == PluginCheckMode::Syntax {
        crate::worker::analyze(path, source)
    } else {
        crate::worker::check(path, source, mode)
    };
    result.unwrap_or_else(|error| PluginCheckReport {
        diagnostics: vec![format!("{error:#}")],
        ..Default::default()
    })
}
