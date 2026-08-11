//! The host half of one agent poll.
//!
//! Thin wrappers over host calls. The parts where drift would be a real bug —
//! classification and the manifest-to-`AgentPane` join — live in `hocus-proto`
//! and are shared. These stay local because sharing them would mean pulling all
//! of hocus's tool dispatch into a daemon that never runs a tool.

use hocus_proto::agents;
use hocus_proto::panes::terminal_panes;
use std::collections::BTreeMap;
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

/// What a pane is showing. Only ever hashed, never shown — but the trailing-blank
/// trim must match hocus's `read_pane` exactly, or the two poll paths would produce
/// different hashes for an unchanged pane and every handover would read as activity.
fn read_pane_text(pane_id: u32) -> String {
    match get_pane_scrollback(PaneId::Terminal(pane_id), false) {
        Ok(contents) => {
            let mut lines = contents.lines_above_viewport;
            lines.extend(contents.viewport);
            lines.extend(contents.lines_below_viewport);
            // Trailing blanks are the unused rows of the pane, not content.
            while lines.last().is_some_and(|line| line.trim().is_empty()) {
                lines.pop();
            }
            if lines.is_empty() {
                "(pane is empty)".to_owned()
            } else {
                lines.join("\n")
            }
        },
        Err(error) => format!("could not read pane: {error}"),
    }
}

/// Fresh pane snapshot, the running command of each pane, and a scrollback hash
/// per running agent for busy/idle.
///
/// Pulled, not pushed. `Event::PaneUpdate` goes only to plugins of the active tab
/// and to background plugins — and while this daemon *is* a background plugin, it
/// polls anyway so that its snapshot is identical to the one hocus computes inline
/// when the daemon is not there.
pub fn gather(
    manual_kinds: &BTreeMap<u32, String>,
    activity: &mut agents::Activity,
) -> Option<Vec<agents::AgentPane>> {
    let session = current_session()?;
    let mut panes = terminal_panes(&session.panes, &session.tabs);
    // ponytail: one blocking host round-trip per terminal pane, every refresh.
    // Fine at a dozen panes; batch it if a session ever gets big enough to feel.
    for pane in &mut panes {
        if let Some(command) = pane_running_command(pane.pane_id) {
            pane.command = Some(command);
        }
        pane.cwd = pane_cwd(pane.pane_id);
    }
    // A pane that closed took its observations with it.
    activity.retain(|id, _| panes.iter().any(|pane| pane.pane_id == *id));
    // ponytail: one read_pane per running agent per poll, to tell busy from idle.
    // Scales with agents, not panes. An agent whose TUI repaints a spinner every
    // frame never reads idle — false "busy" is the safe direction.
    let running: Vec<u32> = panes
        .iter()
        .filter(|pane| {
            pane.status == agents::AgentStatus::Running
                && agents::classify(pane, manual_kinds).is_some()
        })
        .map(|pane| pane.pane_id)
        .collect();
    for pane_id in running {
        agents::observe(activity, pane_id, &read_pane_text(pane_id));
    }
    Some(panes)
}
