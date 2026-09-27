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
//! Everything under a tab: the `Pane` trait and its terminal and plugin implementations, the
//! tiled, floating and overlay pane managers, frames and boundaries, `Tab` itself and its layout
//! applier, and pane groups. Talks to the other server threads only through [`zellij_bus`]; the
//! screen, which owns the tabs, sits above this crate in `zellij-screen`.

pub mod pane_groups;
pub mod panes;
pub mod tab;
pub mod ui;

// Moved files keep their `crate::` paths through these re-exports and aliases, mirroring what
// `zellij-server` does at its root.
pub use zellij_bus::pty_writer::PtyWriteInstruction;
pub use zellij_bus::route::NotificationEnd;
pub use zellij_bus::ServerInstruction;
pub(crate) use zellij_bus::{
    background_jobs, os_input_output, plugins, pty, pty_writer, route, screen, thread_bus,
};
pub use zellij_runtime::channels;
pub use zellij_terminal::{host_query, output};
pub use zellij_types::internal_ids::{ClientId, PluginId, TerminalId};
