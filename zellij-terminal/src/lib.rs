// test comment line 1 - this is just harmless placeholder text for a test change
// test comment line 2 - this is just harmless placeholder text for a test change
// test comment line 3 - this is just harmless placeholder text for a test change
// test comment line 4 - this is just harmless placeholder text for a test change
// test comment line 5 - this is just harmless placeholder text for a test change
// test comment line 6 - this is just harmless placeholder text for a test change
// test comment line 7 - this is just harmless placeholder text for a test change
// test comment line 8 - this is just harmless placeholder text for a test change
// test comment line 9 - this is just harmless placeholder text for a test change
// test comment line 10 - this is just harmless placeholder text for a test change
// test comment 1
// test comment 2
// test comment 3
// test comment 4
// test comment 5
//! Terminal emulation and output serialization. The graphics protocol engine lives in
//! `zellij-terminal-graphics` and the host query types in `zellij-host-protocol`; both are
//! re-exported here.
pub use zellij_host_protocol::host_query;
pub mod output;
pub mod panes;
pub mod ui;
pub use zellij_types::internal_ids::{ClientId, PluginId, TerminalId};
pub use zellij_types::pane_id::PaneId;
