//! `Tab`: one tab of a session. It composes the pane layer's tiled, floating and overlay managers
//! and owns what spans them: focus and fullscreen per client, suppressed panes, pane tabs, swap
//! layouts, mouse handling, pane groups and the clipboard. Talks to the other server threads only
//! through [`yinz_bus`]; the screen, which owns the tabs, sits above this crate in
//! `yinz-screen`.

pub mod pane_groups;
pub mod tab;

// The moved files keep their `crate::` paths through these aliases, as they had in yinz-panes.
pub(crate) use yinz_bus::{
    background_jobs, os_input_output, plugins, pty, pty_writer, route, screen, thread_bus,
    ServerInstruction,
};
pub(crate) use yinz_panes::{panes, ui};
pub(crate) use yinz_terminal::output;
pub(crate) use yinz_types::internal_ids::ClientId;

#[cfg(test)]
pub(crate) use yinz_types::internal_ids::{PluginId, TerminalId};
