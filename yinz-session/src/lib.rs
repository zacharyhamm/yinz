//! Session discovery, lifecycle, resurrection, and reconstruction.
pub mod metadata;
pub mod session_serialization;
#[cfg(not(target_family = "wasm"))]
pub mod sessions;
pub mod warped_tab;

#[cfg(test)]
#[path = "unit/git_plugins.rs"]
mod git_plugins;

#[cfg(test)]
#[path = "unit/file_plugins.rs"]
mod file_plugins;
