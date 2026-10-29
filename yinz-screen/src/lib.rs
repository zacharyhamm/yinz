//! The yinz server's screen thread.
//!
//! [`screen_thread_main`] runs the compositor: it owns every tab, tracks focus, overlays and
//! modes per client, answers host queries, and renders each client's view. It reaches the pty,
//! plugin and server threads only through the instructions on `yinz_bus`, and never names the
//! plugin runtime or the session bootstrap.

pub(crate) use yinz_host_protocol::notifications;
mod screen;

pub use screen::screen_thread_main;

// The moved files keep their `crate::` paths to shared items through these aliases, as the
// server's root did. `plugins` is the bus's instruction module, not the plugin runtime: the
// screen talks to plugins only through `PluginInstruction`.
pub(crate) use yinz_bus::{
    background_jobs, global_async_runtime, nested_guest, os_input_output, plugins, pty, pty_writer,
    session_layout_metadata, thread_bus, warp, ServerInstruction,
};
pub(crate) use yinz_panes::{panes, ui};
pub(crate) use yinz_tab::{pane_groups, tab};
pub(crate) use yinz_terminal::{host_query, output};
pub(crate) use yinz_types::internal_ids::{ClientId, TerminalId};

/// The action dispatcher and its completion types, under the path the server gave them.
mod route {
    pub(crate) use yinz_bus::route::*;
    pub(crate) use yinz_routing::*;
}

#[cfg(test)]
pub(crate) use {
    route::NotificationEnd, thread_bus::ThreadSenders, yinz_runtime::channels,
    yinz_types::internal_ids::PluginId,
};
