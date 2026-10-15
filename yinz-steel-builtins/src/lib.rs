//! Builtin plugins implemented in Steel Scheme and embedded in the server.
use include_dir::{include_dir, Dir};

pub static BUILTIN_STEEL_FILES: Dir = include_dir!("$CARGO_MANIFEST_DIR/assets/plugins");

/// The source of a builtin plugin, without filesystem fallback.
pub fn builtin_steel_plugin(tag: &str) -> Option<&'static str> {
    BUILTIN_STEEL_FILES
        .get_file(format!("{tag}.scm"))?
        .contents_utf8()
}

#[cfg(test)]
#[path = "unit/shared.rs"]
mod shared_tests;

#[cfg(test)]
#[path = "unit/plugins.rs"]
mod tests;

#[cfg(test)]
#[path = "unit/palette.rs"]
mod palette_tests;

#[cfg(test)]
#[path = "unit/configuration.rs"]
mod configuration_tests;

#[cfg(test)]
#[path = "unit/agent_exit.rs"]
mod agent_exit_tests;

#[cfg(test)]
#[path = "unit/bars.rs"]
mod bar_tests;

#[cfg(test)]
#[path = "unit/status_bar.rs"]
mod status_bar_tests;

#[cfg(test)]
#[path = "unit/compact_bar.rs"]
mod compact_bar_tests;

#[cfg(test)]
#[path = "unit/bar_tips.rs"]
mod bar_tips_tests;

#[cfg(test)]
#[path = "unit/strider.rs"]
mod strider_tests;

#[cfg(test)]
#[path = "unit/fixture.rs"]
mod fixture_tests;

#[cfg(test)]
#[path = "unit/file_finder.rs"]
mod file_finder_tests;

#[cfg(test)]
#[path = "unit/configuration_ui.rs"]
mod configuration_ui_tests;

#[cfg(test)]
#[path = "unit/file_finder_listing.rs"]
mod file_finder_listing_tests;

#[cfg(test)]
#[path = "unit/plugin_manager.rs"]
mod plugin_manager_tests;

#[cfg(test)]
#[path = "unit/layout_manager.rs"]
mod layout_manager_tests;

#[cfg(test)]
#[path = "unit/agent_service.rs"]
mod agent_service_tests;

#[cfg(test)]
#[path = "unit/project_store.rs"]
mod project_store_tests;

#[cfg(test)]
#[path = "unit/todos.rs"]
mod todos_tests;
