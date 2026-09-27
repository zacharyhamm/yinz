//! The zellij server's pty thread.
//!
//! [`pty_thread_main`] runs the actor that spawns and supervises every pane's process: local and
//! remote spawns, cwd and git probes, rerun and hold, and releasing or adopting terminals for a
//! warp. It is not the per-pane supervisor (that is `zellij_process::pty_host`) nor the
//! instruction enum (`zellij_bus::pty`); it talks to the other threads only over the bus, and
//! names no pane, tab, terminal emulator, router or plugin runtime.

mod pty;
mod terminal_bytes;

pub use pty::{pty_thread_main, Pty};

// The moved files keep their `crate::` paths to shared items through these aliases, as the
// server's root did. `plugins` is the bus's instruction module, not the plugin runtime.
pub(crate) use zellij_bus::{
    background_jobs, global_async_runtime, os_input_output, plugins, pty_host, route, screen,
    session_layout_metadata, session_recorder, thread_bus, ServerInstruction,
};
pub(crate) use zellij_types::internal_ids::ClientId;
#[cfg(test)]
pub(crate) use zellij_types::internal_ids::TerminalId;
