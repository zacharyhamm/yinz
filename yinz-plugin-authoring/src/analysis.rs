//! The client side of plugin analysis. The SWC visitor that finds grants, lint findings and
//! the outline runs in the compilation worker (`yinz_plugin_host`'s `deno::analysis`).
use std::path::Path;
use yinz_types::authoring::{PluginCheckMode, PluginCheckReport};

/// Syntax checks parse in one worker; the other modes also type-check and run the strict
/// authoring tools. A worker that fails is reported as a diagnostic.
pub fn check(path: &Path, source: &str, mode: PluginCheckMode) -> PluginCheckReport {
    check_with_root(path, source, mode, None)
}

pub fn check_with_root(
    path: &Path,
    source: &str,
    mode: PluginCheckMode,
    root: Option<&Path>,
) -> PluginCheckReport {
    let result = if mode == PluginCheckMode::Syntax {
        crate::worker::analyze(path, source)
    } else {
        if root.is_none() {
            crate::worker::check(path, source, mode)
        } else {
            crate::worker::check_with_root(path, source, mode, root)
        }
    };
    result.unwrap_or_else(|error| PluginCheckReport {
        diagnostics: vec![format!("{error:#}")],
        ..Default::default()
    })
}
