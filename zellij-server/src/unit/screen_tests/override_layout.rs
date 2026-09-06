use super::*;
use zellij_types::input::layout::TabLayoutInfo;

fn layout_at(position: usize, panes: usize) -> TabLayoutInfo {
    TabLayoutInfo {
        tab_index: TabPosition::new(position),
        tab_name: Some(format!("layout-{position}")),
        tiled_layout: TiledPaneLayout {
            children: vec![TiledPaneLayout::default(); panes],
            ..Default::default()
        },
        floating_layouts: vec![],
        swap_tiled_layouts: None,
        swap_floating_layouts: None,
    }
}

fn dispatch_override(
    screen: &mut Screen,
    layouts: Vec<TabLayoutInfo>,
    focused_only: bool,
) -> Vec<(TabId, TabLayoutInfo)> {
    let (tx, rx) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(tx));
    screen
        .override_layout(
            None,
            None,
            layouts,
            true,
            true,
            focused_only,
            ClientId::new(1),
            None,
        )
        .unwrap();
    rx.try_iter()
        .find_map(|(instruction, _)| match instruction {
            PluginInstruction::OverrideLayout(_, _, layouts, true, true, _, _) => Some(layouts),
            _ => None,
        })
        .expect("override was dispatched to plugins")
}

#[test]
fn override_layout_uses_reordered_positions_and_closes_only_surplus_tabs() {
    let mut screen = screen_with_diverged_tab_ids();
    new_tab(&mut screen, 5, TabId::new(4));
    screen
        .move_tab_by_id(TabId::new(3), Direction::Left)
        .unwrap();
    let retained_tiled = PaneId::Terminal(TerminalId::new(40));
    let retained_floating = PaneId::Terminal(TerminalId::new(41));
    add_pane_to_tab(&mut screen, TabId::new(3), retained_tiled, false);
    add_pane_to_tab(&mut screen, TabId::new(3), retained_floating, true);

    let mut first_layout = layout_at(0, 1);
    first_layout
        .floating_layouts
        .push(FloatingPaneLayout::default());
    let layouts = dispatch_override(&mut screen, vec![first_layout, layout_at(1, 2)], false);

    assert_eq!(
        layouts.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        vec![TabId::new(3), TabId::new(2)]
    );
    assert_eq!(
        screen.tabs.keys().copied().collect::<Vec<_>>(),
        vec![TabId::new(2), TabId::new(3)]
    );
    assert_eq!(screen.tabs[&TabId::new(3)].name, "layout-0");
    assert_eq!(screen.tabs[&TabId::new(2)].name, "layout-1");
    assert!(layouts[0].1.floating_layouts[0].already_running);

    for (id, layout) in layouts {
        assert_eq!(layout.tiled_layout.run_instructions_to_ignore, vec![None]);
        let new_pids = if id == TabId::new(2) {
            vec![(TerminalId::new(42), None)]
        } else {
            vec![]
        };
        screen
            .tabs
            .get_mut(&id)
            .unwrap()
            .override_layout(
                layout.tiled_layout,
                layout.floating_layouts,
                layout.swap_tiled_layouts,
                layout.swap_floating_layouts,
                new_pids,
                vec![],
                HashMap::new(),
                true,
                true,
                ClientId::new(1),
                None,
            )
            .unwrap();
    }
    let first_tab = &screen.tabs[&TabId::new(3)];
    assert!(first_tab.has_pane_with_pid(&PaneId::Terminal(TerminalId::new(4))));
    assert!(
        first_tab.has_pane_with_pid(&retained_tiled),
        "retain keeps the extra pane"
    );
    assert!(first_tab.pane_id_is_floating(&retained_floating));
    let second_tab = &screen.tabs[&TabId::new(2)];
    assert!(second_tab.has_pane_with_pid(&PaneId::Terminal(TerminalId::new(3))));
    assert!(second_tab.has_pane_with_pid(&PaneId::Terminal(TerminalId::new(42))));
}

#[test]
fn override_layout_reserves_distinct_ids_for_multiple_new_tabs() {
    let mut screen = screen_with_diverged_tab_ids();
    let layouts = dispatch_override(
        &mut screen,
        (0..4).map(|p| layout_at(p, 1)).collect(),
        false,
    );
    assert_eq!(
        layouts.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        vec![TabId::new(2), TabId::new(3), TabId::new(4), TabId::new(5)]
    );
    for (position, (id, layout)) in layouts.iter().enumerate() {
        assert_eq!(layout.tab_index, TabPosition::new(position));
        let tab = &screen.tabs[id];
        assert_eq!(tab.position, TabPosition::new(position));
        assert_eq!(tab.name, format!("layout-{position}"));
        assert_eq!(tab.is_pending(), position >= 2);
    }
    // A normal NewTab arriving before completion must not reuse an override's ID.
    let next_id = screen.get_new_tab_id();
    assert_eq!(next_id, TabId::new(6));
    new_tab(&mut screen, 10, next_id);
    assert_eq!(screen.tabs.len(), 5);
}

#[test]
fn override_layout_focused_only_targets_active_id_and_keeps_other_tabs() {
    let mut screen = screen_with_diverged_tab_ids();
    assert_eq!(
        screen.get_active_tab(ClientId::new(1)).unwrap().id,
        TabId::new(3)
    );
    let other_name = screen.tabs[&TabId::new(2)].name.clone();
    let layouts = dispatch_override(&mut screen, vec![layout_at(0, 1), layout_at(1, 2)], true);
    assert_eq!(layouts.len(), 1);
    assert_eq!(layouts[0].0, TabId::new(3));
    assert_eq!(layouts[0].1.tab_index, TabPosition::new(0));
    assert_eq!(screen.tabs.len(), 2);
    assert_eq!(screen.tabs[&TabId::new(3)].name, "layout-0");
    assert_eq!(screen.tabs[&TabId::new(2)].name, other_name);
    assert!(screen.tabs[&TabId::new(2)].has_pane_with_pid(&PaneId::Terminal(TerminalId::new(3))));
    assert_eq!(
        layouts[0].1.tiled_layout.run_instructions_to_ignore,
        vec![None]
    );
}
