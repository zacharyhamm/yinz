use super::*;
use yinz_tile::prelude::PaneInfo;

#[test]
fn terminal_discovery_includes_hidden_list_members_only() {
    let mut manifest = PaneManifest::default();
    manifest.panes.insert(
        TabPosition::new(0),
        vec![
            PaneInfo {
                id: yinz_tile::prelude::PaneId::from_legacy(1, false),
                is_suppressed: true,
                is_stack_list_member: true,
                ..Default::default()
            },
            PaneInfo {
                id: yinz_tile::prelude::PaneId::from_legacy(2, false),
                is_suppressed: true,
                cwd: Some("/work/hidden".to_owned()),
                exited: true,
                ..Default::default()
            },
            PaneInfo {
                id: yinz_tile::prelude::PaneId::from_legacy(3, true),
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
    let suppressed = suppressed_terminal_panes(&manifest, &[tab(0, true, false)]);
    assert_eq!(suppressed.len(), 1, "no plugins or duplicate stack members");
    assert_eq!(suppressed[0].pane_id, 2);
    assert_eq!(suppressed[0].cwd.as_deref(), Some("/work/hidden"));
    assert_eq!(suppressed[0].status, agents::AgentStatus::Exited);
    assert_eq!(suppressed[0].tab.as_ref().unwrap().0, TabId::new(100));
}

#[test]
fn agent_panes_omit_the_planners_hook_wiring() {
    let hooks = launch::claude_hook_settings_argument();
    let raw = format!("env YINZ_CLAUDE_INLINE_HOOKS=1 claude --model haiku --settings {hooks}");
    let raw_pane = agent_pane(
        &PaneInfo {
            id: yinz_tile::prelude::PaneId::from_legacy(4, false),
            title: raw.clone(),
            terminal_command: Some(raw.clone()),
            ..Default::default()
        },
        None,
    );
    assert_eq!(raw_pane.command.as_deref(), Some(raw.as_str()));
    let pane = displayed(raw_pane);
    assert_eq!(pane.command.as_deref(), Some("claude --model haiku"));
    assert_eq!(pane.title, "claude --model haiku");
    assert_eq!(
        agents::kind_from_command(pane.command.as_deref().unwrap()).as_deref(),
        Some("claude")
    );
    let titled = displayed(agent_pane(
        &PaneInfo {
            title: "claude (haiku)".to_owned(),
            terminal_command: Some(format!("claude --settings {hooks}")),
            ..Default::default()
        },
        None,
    ));
    assert_eq!(titled.title, "claude (haiku)");
    assert_eq!(titled.command.as_deref(), Some("claude"));
}

#[test]
fn older_snapshots_default_to_no_suppressed_panes() {
    let reply: crate::wire::PollReply =
        serde_json::from_str(r#"{"panes":[],"activity":{}}"#).unwrap();
    assert!(reply.suppressed_panes.is_empty());
    assert_eq!(reply.last_focused_agent, None);
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
                id: yinz_tile::prelude::PaneId::from_legacy(3, false),
                ..PaneInfo::default()
            },
            PaneInfo {
                id: yinz_tile::prelude::PaneId::from_legacy(4, false),
                is_suppressed: true,
                ..PaneInfo::default()
            },
            PaneInfo {
                id: yinz_tile::prelude::PaneId::from_legacy(5, true),
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
        id: yinz_tile::prelude::PaneId::from_legacy(id, false),
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

/// Focusing agent itself must not answer "agents".
#[test]
fn a_plugin_pane_is_never_the_focused_pane() {
    let mut manifest = PaneManifest::default();
    manifest.panes.insert(
        TabPosition::new(0),
        vec![PaneInfo {
            id: yinz_tile::prelude::PaneId::from_legacy(3, true),
            ..focused(3)
        }],
    );

    assert_eq!(focused_pane(&manifest, &[tab(0, true, false)]), None);
}

#[test]
fn pane_hook_recency_comes_from_the_host_manifest() {
    let pane = PaneInfo {
        last_hook_at_ms: Some(123),
        ..Default::default()
    };
    assert_eq!(
        crate::panes::agent_pane(&pane, None).last_hook_at_ms,
        Some(123)
    );
}
