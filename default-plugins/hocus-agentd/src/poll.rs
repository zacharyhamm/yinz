//! The host half of one agent poll.
//!
//! Thin wrappers over host calls. The parts where drift would be a real bug —
//! classification and the manifest-to-`AgentPane` join — live in `hocus-proto`
//! and are shared. These stay local because sharing them would mean pulling all
//! of hocus's tool dispatch into a daemon that never runs a tool.

use hocus_proto::agents;
use hocus_proto::panes::terminal_panes;
use zellij_tile::prelude::*;

fn current_session() -> Option<SessionInfo> {
    get_session_list()
        .ok()?
        .live_sessions
        .into_iter()
        .find(|session| session.is_current_session)
}

/// What a pane is running right now, as one command line.
///
/// `PaneInfo.terminal_command` is only what a pane was *launched* with, so it is
/// `None` for every pane the user opened as a shell. The host resolves this one from
/// the pane's foreground process instead, which is what detection actually wants.
fn pane_running_command(pane_id: u32) -> Option<String> {
    get_pane_running_command(PaneId::Terminal(pane_id))
        .ok()
        .filter(|args| !args.is_empty())
        .map(|args| args.join(" "))
}

fn pane_cwd(pane_id: u32) -> Option<String> {
    get_pane_cwd(PaneId::Terminal(pane_id))
        .ok()
        .map(|path| path.to_string_lossy().into_owned())
}

/// One startup snapshot for panes that existed before this plugin loaded.
///
/// Later changes come from `PaneUpdate`, `CommandChanged`, and `CwdChanged`; this
/// is deliberately not a repeating poll.
pub fn gather() -> Option<Vec<agents::AgentPane>> {
    let session = current_session()?;
    let mut panes = terminal_panes(&session.panes, &session.tabs);
    // ponytail: one blocking host round-trip per terminal pane at plugin startup.
    // Events keep this fresh afterwards; batch only if startup ever feels slow.
    for pane in &mut panes {
        if let Some(command) = pane_running_command(pane.pane_id) {
            pane.command = Some(command);
        }
        pane.cwd = pane_cwd(pane.pane_id);
    }
    Some(panes)
}
