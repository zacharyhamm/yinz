//! The client side of plugin analysis. Grant inference and the outline run in
//! the supervised Steel compilation worker.
use std::path::Path;
use yinz_types::authoring::{PluginCheckMode, PluginCheckReport};

/// Syntax and formatting parse the named file in a worker. Semantic and prepare checks also
/// compile confined imports. A worker that fails is reported as a diagnostic.
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
