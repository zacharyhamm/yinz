//! The path of the running Yinz executable.

use std::path::PathBuf;

/// Resolve before a package upgrade unlinks the running executable. Shared by
/// supervisors, hook payloads and plugin helpers that re-exec this server's binary.
pub fn server_binary() -> Option<&'static PathBuf> {
    static BINARY: std::sync::LazyLock<Option<PathBuf>> =
        std::sync::LazyLock::new(|| match std::env::current_exe() {
            Ok(path) => Some(path),
            Err(e) => {
                log::warn!("Could not resolve our own binary ({e}); re-execution is unavailable.");
                None
            },
        });
    BINARY.as_ref()
}
