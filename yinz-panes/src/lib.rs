//! The pane layer under a tab: the `Pane` trait and its terminal and plugin implementations, the
//! tiled, floating and overlay pane managers, frames and boundaries, and the layout applier that
//! fills those managers from a layout. Talks to the other server threads only through
//! [`yinz_bus`]; `Tab`, which composes these managers, sits above this crate in `yinz-tab`.

pub mod layout_applier;
pub mod pane;
pub mod pane_tabs;
pub mod panes;
pub mod ui;

#[cfg(test)]
#[path = "./unit/layout_applier_tests/mod.rs"]
mod layout_applier_tests;

// Moved files keep their `crate::` paths through these re-exports and aliases, mirroring what
// `yinz-server` does at its root.
pub use yinz_bus::pty_writer::PtyWriteInstruction;
pub use yinz_bus::route::NotificationEnd;
pub(crate) use yinz_bus::{os_input_output, plugins, pty, route, thread_bus};
pub use yinz_runtime::channels;
pub use yinz_terminal::{host_query, output};
pub use yinz_types::internal_ids::{ClientId, PluginId, TerminalId};
