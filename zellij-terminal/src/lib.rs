//! Terminal emulation, graphics, and output serialization.
pub mod host_query;
pub mod output;
pub mod panes;
pub mod ui;
pub use zellij_types::internal_ids::{ClientId, PluginId, TerminalId};
pub use zellij_types::pane_id::PaneId;
