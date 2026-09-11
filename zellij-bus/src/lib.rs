//! The zellij server's thread bus.
//!
//! Every server thread owns one instruction enum and sends the others' through
//! [`thread_bus::ThreadSenders`]. This crate holds those enums, the payload types they carry, the
//! bus itself, and the code that only ever talks over it: action routing, warps, background jobs,
//! the session recorder and the OS boundary. Nothing here names a tab, a pane, a screen, or the
//! plugin runtime.

#[cfg(test)]
#[path = "./unit/instruction_context_tests.rs"]
mod instruction_context_tests;

#[path = "os_input_output_unix.rs"]
pub mod os_input_output_unix;

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
pub use zellij_process::{firecracker, pty_host};
pub use zellij_runtime::channels;
pub use zellij_terminal::{host_query, output};
pub use zellij_types::internal_ids::{ClientId, PluginId, TerminalId};
pub use zellij_types::pane_id::PaneId;
