//! Turning a host pane manifest into the shape agent detection wants.
//!
//! Only hocus-agentd builds panes from a manifest; hocus and hocus-store read
//! the result off the wire. It lives here rather than in the daemon because the
//! shape it produces is the shape all three agree on, and a type that crosses
//! the wire belongs with the other wire types rather than inside one end of it.

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
            // The manifest carries the folder now. It used to say nothing, so
            // the only word on where a pane was came from `CwdChanged` — heard
            // once, and never again by anyone who missed it.
            cwd: pane.cwd.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use zellij_tile::prelude::PaneInfo;

    #[test]
    fn terminal_panes_carry_the_folder_the_host_reported() {
        let mut manifest = PaneManifest::default();
        manifest.panes.insert(
            0,
            vec![
                PaneInfo {
                    id: 3,
                    cwd: Some("/work/hocus".to_owned()),
                    ..PaneInfo::default()
                },
                PaneInfo {
                    id: 4,
                    cwd: None,
                    ..PaneInfo::default()
                },
            ],
        );

        let panes = terminal_panes(&manifest, &[]);

        assert_eq!(panes[0].cwd.as_deref(), Some("/work/hocus"));
        assert_eq!(panes[1].cwd, None);
    }
}
