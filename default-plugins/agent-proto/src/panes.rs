//! Turning a host pane manifest into the shape agent detection wants.
//!
//! agent-service builds the visible list; an explicit pane lookup uses the same
//! conversion for hidden sources. agent and project-store read the result off the wire. It lives here rather than in the daemon because the
//! shape it produces is the shape all three agree on, and a type that crosses
//! the wire belongs with the other wire types rather than inside one end of it.

use crate::agents;
use crate::launch;
use std::collections::{BTreeMap, BTreeSet};
use yinz_tile::prelude::TabId;
use yinz_tile::prelude::TabPosition;
use yinz_tile::prelude::{PaneInfo, PaneManifest, TabInfo};

/// The terminal panes of a manifest, as the agent module wants them.
///
/// Plugin panes are dropped here rather than checked later: an agent is a program at a
/// prompt, and Agent itself is a plugin pane. Hidden stack-list terminals remain
/// discoverable through their list entries. Other suppressed panes are excluded.
pub fn terminal_panes(manifest: &PaneManifest, tabs: &[TabInfo]) -> Vec<agents::AgentPane> {
    terminal_panes_by_visibility(manifest, tabs, false)
}

/// Hidden terminals omitted from the normal agent list. Stack members are already in it.
pub fn suppressed_terminal_panes(
    manifest: &PaneManifest,
    tabs: &[TabInfo],
) -> Vec<agents::AgentPane> {
    terminal_panes_by_visibility(manifest, tabs, true)
}

fn terminal_panes_by_visibility(
    manifest: &PaneManifest,
    tabs: &[TabInfo],
    suppressed: bool,
) -> Vec<agents::AgentPane> {
    // The manifest is keyed by tab *position*, not tab id — same join as
    // tools::list_session.
    let by_position: BTreeMap<TabPosition, (TabId, &str)> = tabs
        .iter()
        .map(|tab| (tab.position, (tab.tab_id, tab.name.as_str())))
        .collect();
    let mut panes: Vec<agents::AgentPane> = manifest
        .panes
        .iter()
        .flat_map(|(position, panes)| panes.iter().map(move |pane| (Some(*position), pane)))
        .filter(|(_, pane)| {
            !pane.is_plugin() && (pane.is_suppressed && !pane.is_stack_list_member) == suppressed
        })
        .map(|(position, pane)| {
            agent_pane(
                pane,
                position
                    .and_then(|position| by_position.get(&position))
                    .map(|(id, name)| (*id, (*name).to_owned())),
            )
        })
        .collect();
    // The manifest is a HashMap keyed by tab position, so its order is not stable.
    // agent_list is read by a model on every turn; give it the same order every time.
    panes.sort_by_key(|pane| pane.pane_id);
    panes
}

/// A pane as shown to a person or a model: its command without the planner's hook wiring,
/// kilobytes per claude pane that say nothing about it. A title that is still the raw
/// command is shortened the same way. Detection reads the full command, so only output
/// goes through this.
pub fn displayed(mut pane: agents::AgentPane) -> agents::AgentPane {
    if let Some(command) = pane.command.take() {
        if pane.title == command {
            pane.title = launch::display_command(&pane.title);
        }
        pane.command = Some(launch::display_command(&command));
    }
    pane
}

/// Convert a terminal's manifest record, including an explicitly requested hidden pane.
pub fn agent_pane(pane: &PaneInfo, tab: Option<(TabId, String)>) -> agents::AgentPane {
    agents::AgentPane {
        pane_id: pane.id.legacy_parts().0,
        title: pane.title.clone(),
        command: pane.terminal_command.clone(),
        // The manifest carries the folder now. It used to say nothing, so
        // the only word on where a pane was came from `CwdChanged` — heard
        // once, and never again by anyone who missed it.
        cwd: pane.cwd.clone(),
        host: pane.host.clone(),
        bubblewrap: pane
            .target
            .bubblewrap_root()
            .map(|path| path.display().to_string()),
        // Order matters: a held pane has also exited, and "waiting to be re-run"
        // is the more useful of the two things to say.
        status: if pane.is_held {
            agents::AgentStatus::Held
        } else if pane.exited {
            agents::AgentStatus::Exited
        } else {
            agents::AgentStatus::Running
        },
        tab,
        // A manifest says nothing about what a pane was running a moment ago.
        // Only the daemon knows that, and it writes it back on afterwards.
        detected: None,
        agent: pane.agent.clone(),
    }
}

/// Every terminal pane the manifest carries, suppressed ones included.
///
/// `terminal_panes` drops the suppressed, which is right for a list the user reads
/// and wrong for deciding a pane has closed. A pane is suppressed while something
/// stands in front of it — the scrollback editor, most often — and it comes back
/// with the same id, still running whatever it was running. Pruning what the daemon
/// knows against the visible list alone threw that away: the folder, the branch
/// probed from it, and the latch that says the pane runs an agent at all.
pub fn manifest_pane_ids(manifest: &PaneManifest) -> BTreeSet<u32> {
    manifest
        .panes
        .values()
        .flatten()
        .filter(|pane| !pane.is_plugin())
        .map(|pane| pane.id.legacy_parts().0)
        .collect()
}

/// The terminal pane the user is looking at, if it is one.
///
/// `PaneInfo::is_focused` is "focused in its layer", so a manifest flags one pane per
/// tab per layer — the answer is only the user's pane once the tab is the active one
/// and the layer is the one on top. A plugin pane is not an answer at all: agent
/// itself is one, and focusing the sidebar returns no focused terminal.
pub fn focused_pane(manifest: &PaneManifest, tabs: &[TabInfo]) -> Option<u32> {
    let active = tabs.iter().find(|tab| tab.active);
    manifest
        .panes
        .iter()
        .filter(|(position, _)| active.map(|tab| tab.position) == Some(**position))
        .flat_map(|(_, panes)| panes.iter())
        .filter(|pane| active.map(|tab| tab.are_floating_panes_visible) == Some(pane.is_floating))
        .find(|pane| {
            pane.is_focused
                && !pane.is_plugin()
                && (!pane.is_suppressed || pane.is_stack_list_member)
        })
        .map(|pane| pane.id.legacy_parts().0)
}

#[cfg(test)]
#[path = "unit/panes.rs"]
mod tests;
