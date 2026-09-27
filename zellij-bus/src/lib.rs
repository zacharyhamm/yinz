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
//! The zellij server's thread bus.
//!
//! Every server thread owns one instruction enum and sends the others' through
//! [`thread_bus::ThreadSenders`]. This crate holds those enums, the payload types they carry, the
//! bus itself, and the code that only ever talks over it: warps, background jobs,
//! the session recorder and the OS boundary's contract ([`os_input_output::ServerOsApi`], which
//! `zellij-server-os` implements). Nothing here names a tab, a pane, a screen, or the plugin
//! runtime.

#[cfg(test)]
#[path = "./unit/instruction_context_tests.rs"]
mod instruction_context_tests;

#[cfg(test)]
#[path = "./unit/pty_exit_tests.rs"]
mod pty_exit_tests;

pub mod background_jobs;
pub mod frame_color_override;
pub mod global_async_runtime;
pub mod loading_indication;
pub mod nested_guest;
pub mod os_input_output;
pub mod plugins;
pub mod pty;
pub mod pty_writer;
pub mod route;
pub mod screen;
pub mod server_instruction;
pub mod session_layout_metadata;
pub mod session_recorder;
pub mod thread_bus;
pub mod warp;

pub use server_instruction::ServerInstruction;

// Moved files keep their `crate::` paths to shared items through these re-exports, mirroring
// what `zellij-server` and `zellij-terminal` do at their roots.
pub use zellij_process::pty_host;
pub use zellij_runtime::channels;
pub use zellij_terminal::host_query;
pub use zellij_types::internal_ids::{ClientId, PluginId, TerminalId};
pub use zellij_types::pane_id::PaneId;
