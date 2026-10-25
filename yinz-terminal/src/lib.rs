//! Terminal emulation and output serialization. The graphics protocol engine lives in
//! `yinz-terminal-graphics` and the host query types in `yinz-host-protocol`; both are
//! re-exported here.
pub use yinz_host_protocol::host_query;
pub mod agent_theme;
pub mod output;
pub mod panes;
pub mod ui;
pub use yinz_types::internal_ids::{ClientId, PluginId, TerminalId};
pub use yinz_types::pane_id::PaneId;
