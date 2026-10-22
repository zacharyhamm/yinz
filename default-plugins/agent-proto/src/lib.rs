//! Pure agent models, planning helpers, and wire types shared by the host and Scheme bridge.
//! Stored schemas and project models come from `project-data`.
//! Guest lifecycle, host commands, and transaction sequencing remain in Scheme.

/// The session's reading of a Claude Code hook event, for plugins that link only this crate.
pub use yinz_tile::agent;
pub mod agents;
pub mod completion;
pub mod decisions;
pub mod envelope;
pub mod launch;
pub mod llm;
pub mod markdown;
pub mod panes;
pub mod run_history;
pub mod tabs;
pub mod text;
pub mod wire;

pub mod panel_text;

pub use project_data::{git_probe, location, paths, projects, store};

pub mod pane_operations;
