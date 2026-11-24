//! Pure terminal widgets, text helpers, and shared models used by the Scheme host.
pub mod clock;
pub mod prelude;
pub mod text;
pub mod ui_components;

/// Scheme plugin naming and editing utilities.
///
/// ```
/// use std::path::Path;
/// use yinz_tile::plugin_authoring::{is_script_plugin, edits::apply_edits};
///
/// assert!(is_script_plugin(Path::new("keys.scm")));
/// assert!(apply_edits("(load)", &[]).is_err());
/// ```
pub use yinz_plugin_authoring::authoring as plugin_authoring;
pub use yinz_project_config as project_config;
pub use yinz_runtime::agent;
