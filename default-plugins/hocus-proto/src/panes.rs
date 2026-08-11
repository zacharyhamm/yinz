//! Turning a host pane manifest into the shape agent detection wants.
//!
//! Shared rather than copied into each plugin because hocus polls inline when
//! the daemon is absent and the daemon polls otherwise, and the two feed one
//! `activity` map. Two implementations that drifted would hash the same
//! unchanged pane differently depending on who polled it, and every handover
//! between them would read as activity.

use crate::agents;
use std::collections::BTreeMap;
use zellij_tile::prelude::{PaneManifest, TabInfo};

/// The terminal panes of a manifest, as the agent module wants them.
///
/// Plugin panes are dropped here rather than checked later: an agent is a program at a
/// prompt, and Hocus itself is a plugin pane. Suppressed panes are dropped for the same
/// reason `list_session` drops them — the user cannot see them. Global panes are in no
/// tab, so they are in their own field and would be missed by the tab walk alone.
pub fn terminal_panes(manifest: &PaneManifest, tabs: &[TabInfo]) -> Vec<agents::AgentPane> {
    // The manifest is keyed by tab *position*, not tab id — same join as
    // tools::list_session.
    let by_position: BTreeMap<usize, (usize, &str)> = tabs
        .iter()
        .map(|tab| (tab.position, (tab.tab_id, tab.name.as_str())))
        .collect();
    let mut panes: Vec<agents::AgentPane> = manifest
        .panes
        .iter()
        .flat_map(|(position, panes)| panes.iter().map(move |pane| (Some(*position), pane)))
        .chain(manifest.global_panes.iter().map(|pane| (None, pane)))
        .filter(|(_, pane)| !pane.is_plugin && !pane.is_suppressed)
        .map(|(position, pane)| agents::AgentPane {
            pane_id: pane.id,
            title: pane.title.clone(),
            command: pane.terminal_command.clone(),
            cwd: None,
            // Order matters: a held pane has also exited, and "waiting to be re-run"
            // is the more useful of the two things to say.
            status: if pane.is_held {
                agents::AgentStatus::Held
            } else if pane.exited {
                agents::AgentStatus::Exited
            } else {
                agents::AgentStatus::Running
            },
            tab: position
                .and_then(|position| by_position.get(&position))
                .map(|(id, name)| (*id, (*name).to_owned())),
        })
        .collect();
    // The manifest is a HashMap keyed by tab position, so its order is not stable.
    // agent_list is read by a model on every turn; give it the same order every time.
    panes.sort_by_key(|pane| pane.pane_id);
    panes
}
