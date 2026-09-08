//! Turning a host pane manifest into the shape agent detection wants.
//!
//! hocus-agentd builds the visible list; an explicit pane lookup uses the same
//! conversion for hidden sources. hocus and hocus-store read the result off the wire. It lives here rather than in the daemon because the
//! shape it produces is the shape all three agree on, and a type that crosses
//! the wire belongs with the other wire types rather than inside one end of it.

use crate::agents;
use std::collections::{BTreeMap, BTreeSet};
use zellij_tile::prelude::TabId;
use zellij_tile::prelude::TabPosition;
use zellij_tile::prelude::{PaneInfo, PaneManifest, TabInfo};

/// The terminal panes of a manifest, as the agent module wants them.
///
/// Plugin panes are dropped here rather than checked later: an agent is a program at a
/// prompt, and Hocus itself is a plugin pane. Hidden stack-list terminals remain
/// discoverable through their list entries. Other suppressed panes are excluded.
pub fn terminal_panes(manifest: &PaneManifest, tabs: &[TabInfo]) -> Vec<agents::AgentPane> {
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
        .filter(|(_, pane)| !pane.is_plugin() && (!pane.is_suppressed || pane.is_stack_list_member))
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
        firecracker_image: pane.target.firecracker_image().map(str::to_owned),
        firecracker_worktree: pane
            .target
            .firecracker_worktree()
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
/// and the layer is the one on top. A plugin pane is not an answer at all: hocus
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
mod tests {
    use super::*;
    use zellij_tile::prelude::{PaneInfo, PaneTarget};

    #[test]
    fn terminal_discovery_includes_hidden_list_members_only() {
        let mut manifest = PaneManifest::default();
        manifest.panes.insert(
            TabPosition::new(0),
            vec![
                PaneInfo {
                    id: zellij_tile::prelude::PaneId::from_legacy(1, false),
                    is_suppressed: true,
                    is_stack_list_member: true,
                    ..Default::default()
                },
                PaneInfo {
                    id: zellij_tile::prelude::PaneId::from_legacy(2, false),
                    is_suppressed: true,
                    ..Default::default()
                },
            ],
        );
        let panes = terminal_panes(&manifest, &[]);
        assert_eq!(
            panes.iter().map(|pane| pane.pane_id).collect::<Vec<_>>(),
            vec![1]
        );
    }

    #[test]
    fn terminal_panes_carry_the_reported_firecracker_location() {
        let mut manifest = PaneManifest::default();
        manifest.panes.insert(
            TabPosition::new(0),
            vec![
                PaneInfo {
                    id: zellij_tile::prelude::PaneId::from_legacy(3, false),
                    cwd: Some("/work/hocus".to_owned()),
                    host: Some("firecracker:review".to_owned()),
                    target: PaneTarget::Firecracker {
                        vm: "review".to_owned(),
                        image: Some("rust-dev".to_owned()),
                        worktree: Some("/host/hocus".into()),
                    },
                    ..PaneInfo::default()
                },
                PaneInfo {
                    id: zellij_tile::prelude::PaneId::from_legacy(4, false),
                    cwd: None,
                    ..PaneInfo::default()
                },
            ],
        );

        let panes = terminal_panes(&manifest, &[]);

        assert_eq!(panes[0].cwd.as_deref(), Some("/work/hocus"));
        assert_eq!(panes[0].firecracker_image.as_deref(), Some("rust-dev"));
        assert_eq!(
            panes[0].firecracker_worktree.as_deref(),
            Some("/host/hocus")
        );
        assert_eq!(panes[1].cwd, None);
    }

    /// A suppressed pane is hidden, not gone. It keeps its id and its process,
    /// so what the daemon knows about it must survive the thing standing in
    /// front of it — which is exactly what pruning against the visible list did
    /// not do.
    #[test]
    fn manifest_pane_ids_include_a_suppressed_pane() {
        let mut manifest = PaneManifest::default();
        manifest.panes.insert(
            TabPosition::new(0),
            vec![
                PaneInfo {
                    id: zellij_tile::prelude::PaneId::from_legacy(3, false),
                    ..PaneInfo::default()
                },
                PaneInfo {
                    id: zellij_tile::prelude::PaneId::from_legacy(4, false),
                    is_suppressed: true,
                    ..PaneInfo::default()
                },
                PaneInfo {
                    id: zellij_tile::prelude::PaneId::from_legacy(5, true),
                    ..PaneInfo::default()
                },
            ],
        );

        let visible: Vec<u32> = terminal_panes(&manifest, &[])
            .iter()
            .map(|pane| pane.pane_id)
            .collect();
        assert_eq!(visible, [3], "the suppressed pane is not one to show");
        assert_eq!(
            manifest_pane_ids(&manifest),
            BTreeSet::from([3, 4]),
            "but it is still a live pane, and the plugin still is not one",
        );
    }

    fn tab(position: usize, active: bool, floating_visible: bool) -> TabInfo {
        TabInfo {
            position: TabPosition::new(position),
            tab_id: TabId::new(position + 100),
            active,
            are_floating_panes_visible: floating_visible,
            ..TabInfo::default()
        }
    }

    fn focused(id: u32) -> PaneInfo {
        PaneInfo {
            id: zellij_tile::prelude::PaneId::from_legacy(id, false),
            is_focused: true,
            ..PaneInfo::default()
        }
    }

    #[test]
    fn the_focused_pane_is_the_one_in_the_active_tab() {
        let mut manifest = PaneManifest::default();
        manifest.panes.insert(TabPosition::new(0), vec![focused(3)]);
        manifest.panes.insert(TabPosition::new(1), vec![focused(4)]);

        let tabs = [tab(0, false, false), tab(1, true, false)];

        assert_eq!(focused_pane(&manifest, &tabs), Some(4));
    }

    #[test]
    fn no_active_tab_means_no_focused_pane() {
        let mut manifest = PaneManifest::default();
        manifest.panes.insert(TabPosition::new(0), vec![focused(3)]);

        assert_eq!(focused_pane(&manifest, &[tab(0, false, false)]), None);
    }

    /// Both layers flag a focused pane. The visible one is the user's.
    #[test]
    fn the_visible_layer_decides_between_two_focused_panes() {
        let mut manifest = PaneManifest::default();
        manifest.panes.insert(
            TabPosition::new(0),
            vec![
                focused(3),
                PaneInfo {
                    is_floating: true,
                    ..focused(4)
                },
            ],
        );

        assert_eq!(focused_pane(&manifest, &[tab(0, true, false)]), Some(3));
        assert_eq!(focused_pane(&manifest, &[tab(0, true, true)]), Some(4));
    }

    /// Focusing hocus itself must not answer "hocus".
    #[test]
    fn a_plugin_pane_is_never_the_focused_pane() {
        let mut manifest = PaneManifest::default();
        manifest.panes.insert(
            TabPosition::new(0),
            vec![PaneInfo {
                id: zellij_tile::prelude::PaneId::from_legacy(3, true),
                ..focused(3)
            }],
        );

        assert_eq!(focused_pane(&manifest, &[tab(0, true, false)]), None);
    }
}
