use super::*;
use yinz_types::input::layout::TabLayoutInfo;
use yinz_types::internal_ids::TabNumber;

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
) -> (Vec<(TabId, TabLayoutInfo)>, Vec<TabId>) {
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
            PluginInstruction::OverrideLayout(_, _, tabs_to_close, layouts, true, true, _, _) => {
                Some((layouts, tabs_to_close))
            },
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
    let (layouts, tabs_to_close) =
        dispatch_override(&mut screen, vec![first_layout, layout_at(1, 2)], false);

    assert_eq!(
        layouts.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        vec![TabId::new(3), TabId::new(2)]
    );
    assert_eq!(
        screen.tabs.keys().copied().collect::<Vec<_>>(),
        vec![TabId::new(2), TabId::new(3), TabId::new(4)]
    );
    assert_ne!(screen.tabs[&TabId::new(3)].name, "layout-0");
    assert_ne!(screen.tabs[&TabId::new(2)].name, "layout-1");
    assert!(layouts[0].1.floating_layouts[0].already_running);

    let results = layouts
        .into_iter()
        .map(|(id, layout)| {
            assert_eq!(layout.tiled_layout.run_instructions_to_ignore, vec![None]);
            crate::screen::TabOverrideResult {
                tab_index: id,
                tab_name: layout.tab_name,
                tiled_layout: layout.tiled_layout,
                floating_layouts: layout.floating_layouts,
                swap_tiled_layouts: layout.swap_tiled_layouts,
                swap_floating_layouts: layout.swap_floating_layouts,
                new_terminal_pids: if id == TabId::new(2) {
                    vec![(TerminalId::new(42), None)]
                } else {
                    vec![]
                },
                new_floating_pane_pids: vec![],
                plugin_ids: HashMap::new(),
            }
        })
        .collect();
    screen
        .complete_override_layout(
            results,
            vec![],
            tabs_to_close,
            true,
            true,
            ClientId::new(1),
            &mut super::super::thread_main::PendingEvents::default(),
        )
        .unwrap();
    assert_eq!(
        screen.tabs.keys().copied().collect::<Vec<_>>(),
        vec![TabId::new(2), TabId::new(3)]
    );
    assert_eq!(screen.tabs[&TabId::new(3)].name, "layout-0");
    assert_eq!(screen.tabs[&TabId::new(2)].name, "layout-1");
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
    let (layouts, tabs_to_close) = dispatch_override(
        &mut screen,
        (0..4).map(|p| layout_at(p, 1)).collect(),
        false,
    );
    assert_eq!(
        layouts.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        vec![TabId::new(2), TabId::new(3), TabId::new(4), TabId::new(5)]
    );
    assert!(tabs_to_close.is_empty());
    for (position, (id, layout)) in layouts.iter().enumerate() {
        assert_eq!(layout.tab_index, TabPosition::new(position));
        let tab = &screen.tabs[id];
        assert_eq!(tab.position, TabPosition::new(position));
        if position >= 2 {
            assert_eq!(tab.name, format!("layout-{position}"));
        } else {
            assert_ne!(tab.name, format!("layout-{position}"));
        }
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
    let (layouts, tabs_to_close) =
        dispatch_override(&mut screen, vec![layout_at(0, 1), layout_at(1, 2)], true);
    assert_eq!(layouts.len(), 1);
    assert_eq!(layouts[0].0, TabId::new(3));
    assert_eq!(layouts[0].1.tab_index, TabPosition::new(0));
    assert_eq!(screen.tabs.len(), 2);
    assert!(tabs_to_close.is_empty());
    assert_ne!(screen.tabs[&TabId::new(3)].name, "layout-0");
    assert_eq!(screen.tabs[&TabId::new(2)].name, other_name);
    assert!(screen.tabs[&TabId::new(2)].has_pane_with_pid(&PaneId::Terminal(TerminalId::new(3))));
    assert_eq!(
        layouts[0].1.tiled_layout.run_instructions_to_ignore,
        vec![None]
    );
}

#[test]
fn override_completion_applies_a_queued_tab_switch() {
    queued_switch_during_override(false, vec![]);
}

#[test]
fn override_completion_preserves_a_queued_switch_while_another_tab_is_pending() {
    queued_switch_during_override(true, vec![]);
}

#[test]
fn override_completion_removes_failed_reservations_and_preserves_established_tabs() {
    queued_switch_during_override(false, vec![TabId::new(0), TabId::new(2), TabId::new(99)]);
}

#[test]
fn failed_override_keeps_navigation_queued_until_an_unrelated_tab_settles() {
    queued_switch_during_override(true, vec![TabId::new(0), TabId::new(2)]);
}

#[test]
fn all_failed_override_completion_releases_queued_navigation() {
    queued_switch_during_override(false, vec![TabId::new(0), TabId::new(1), TabId::new(2)]);
}

fn queued_switch_during_override(another_tab_pending: bool, failed_tabs: Vec<TabId>) {
    let mut mock_screen = MockScreen::new(Size { cols: 80, rows: 24 });
    let screen_thread = mock_screen.run(None, vec![]);
    let client_id = mock_screen.main_client_id;
    // Two established tabs let an all-failed override preserve a navigation target.
    mock_screen
        .to_screen
        .send(ScreenInstruction::NewTab(
            None,
            None,
            Some(TiledPaneLayout::default()),
            vec![],
            None,
            (Some(vec![]), Some(vec![])),
            None,
            false,
            false,
            client_id,
            None,
        ))
        .unwrap();
    mock_screen
        .to_screen
        .send(ScreenInstruction::ApplyLayout(
            TiledPaneLayout::default(),
            vec![],
            vec![(TerminalId::new(1), None)],
            vec![],
            vec![],
            HashMap::new(),
            TabId::new(1),
            false,
            client_id,
            None,
            None,
        ))
        .unwrap();
    mock_screen
        .to_screen
        .send(ScreenInstruction::OverrideLayout(
            None,
            None,
            vec![layout_at(0, 1), layout_at(1, 1), layout_at(2, 1)],
            true,
            true,
            false,
            client_id,
            None,
        ))
        .unwrap();
    // Stand in for the plugin/PTY threads, using the layouts the screen dispatched.
    let deadline = std::time::Instant::now() + yinz_test_support::default_timeout();
    let (layouts, tabs_to_close) = loop {
        let (instruction, _) = mock_screen
            .plugin_receiver
            .as_ref()
            .unwrap()
            .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
            .expect("the screen dispatched the override");
        if let PluginInstruction::OverrideLayout(_, _, tabs_to_close, layouts, _, _, _, _) =
            instruction
        {
            break (layouts, tabs_to_close);
        }
    };
    if another_tab_pending {
        mock_screen
            .to_screen
            .send(ScreenInstruction::NewTab(
                None,
                None,
                Some(TiledPaneLayout::default()),
                vec![],
                None,
                (Some(vec![]), Some(vec![])),
                None,
                false,
                false,
                client_id,
                None,
            ))
            .unwrap();
    }
    // Switch, completion and queries share a FIFO; no render timing is involved.
    mock_screen
        .to_screen
        .send(ScreenInstruction::GoToTab(
            TabNumber::new(2),
            Some(client_id),
            None,
        ))
        .unwrap();
    let focused_tab = || {
        let (tx, rx) = crossbeam::channel::bounded(1);
        mock_screen
            .to_screen
            .send(ScreenInstruction::GetCurrentTabInfo {
                client_id,
                response_channel: tx,
            })
            .unwrap();
        rx.recv_timeout(yinz_test_support::default_timeout())
            .expect("the screen processed the query")
            .expect("a focused tab")
            .tab_id
    };
    let before_completion = focused_tab();
    let results = layouts
        .into_iter()
        .filter(|(tab_index, _)| !failed_tabs.contains(tab_index))
        .map(|(tab_index, layout)| crate::screen::TabOverrideResult {
            tab_index,
            tab_name: layout.tab_name,
            tiled_layout: layout.tiled_layout,
            floating_layouts: layout.floating_layouts,
            swap_tiled_layouts: layout.swap_tiled_layouts,
            swap_floating_layouts: layout.swap_floating_layouts,
            new_terminal_pids: if tab_index != TabId::new(2) {
                vec![] // Established tabs retain their existing terminals.
            } else {
                vec![(TerminalId::new(2), None)]
            },
            new_floating_pane_pids: vec![],
            plugin_ids: HashMap::new(),
        })
        .collect();
    mock_screen
        .to_screen
        .send(ScreenInstruction::OverrideLayoutComplete(
            results,
            failed_tabs.clone(),
            tabs_to_close,
            true,
            true,
            client_id,
            None,
        ))
        .unwrap();
    let after_override = focused_tab();
    if another_tab_pending {
        mock_screen
            .to_screen
            .send(ScreenInstruction::ApplyLayout(
                TiledPaneLayout::default(),
                vec![],
                vec![(TerminalId::new(3), None)],
                vec![],
                vec![],
                HashMap::new(),
                TabId::new(3),
                false,
                client_id,
                None,
                None,
            ))
            .unwrap();
    }
    let after_all_layouts = focused_tab();
    let tab_info = |tab_id| {
        let (tx, rx) = crossbeam::channel::bounded(1);
        mock_screen
            .to_screen
            .send(ScreenInstruction::GetTabInfo {
                tab_id,
                response_channel: tx,
            })
            .unwrap();
        rx.recv_timeout(yinz_test_support::default_timeout())
            .unwrap()
    };
    let established_tabs = [tab_info(TabId::new(0)), tab_info(TabId::new(1))];
    let reservation = tab_info(TabId::new(2));
    let unrelated_tab = tab_info(TabId::new(3));
    mock_screen.teardown(vec![screen_thread]);
    assert!(established_tabs.iter().all(Option::is_some));
    assert_eq!(reservation.is_none(), !failed_tabs.is_empty());
    assert_eq!(unrelated_tab.is_some(), another_tab_pending);
    assert_eq!(before_completion, TabId::new(0));
    assert_eq!(
        after_override,
        TabId::new(if another_tab_pending { 0 } else { 1 })
    );
    assert_eq!(after_all_layouts, TabId::new(1));
}

fn unfit_layout() -> TiledPaneLayout {
    TiledPaneLayout {
        children: vec![
            TiledPaneLayout {
                split_size: Some(yinz_types::geometry::SplitSize::Fixed(10_000)),
                ..Default::default()
            },
            TiledPaneLayout::default(),
        ],
        ..Default::default()
    }
}

#[test]
fn rejected_layout_preserves_panes_moved_into_a_pending_tab() {
    for placement in ["multiple", "single", "adopted"] {
        let mut screen = screen_with_diverged_tab_ids();
        let client = ClientId::new(1);
        let active = screen.get_active_tab(client).unwrap().id;
        let terminal = TerminalId::new(50);
        let pane_id = PaneId::Terminal(terminal);
        let pending_tab = if placement == "adopted" {
            let (_, _, target) = adopted_pane_new_pane(
                &mut screen,
                yinz_types::data::WarpPlacement::NewTab,
                client,
                RunCommand::default(),
            );
            let ClientTabIndexOrPaneId::TabIndex(tab_id) = target else {
                panic!("adopted pane must target its new tab");
            };
            add_pane_to_tab(&mut screen, tab_id, pane_id, false);
            tab_id
        } else {
            add_pane_to_tab(&mut screen, active, pane_id, false);
            if placement == "multiple" {
                screen
                    .break_multiple_panes_to_new_tab(vec![pane_id], None, true, None, client)
                    .unwrap()
            } else {
                screen
                    .get_active_tab_mut(client)
                    .unwrap()
                    .focus_pane_with_id(pane_id, false, false, client)
                    .unwrap();
                let tab_id = screen.get_new_tab_id();
                screen
                    .break_pane(None, screen.default_layout.clone(), client)
                    .unwrap();
                tab_id
            }
        };
        let tab = screen.tabs.get_mut(&pending_tab).unwrap();
        assert!(tab.is_pending(), "{placement}");
        tab.handle_pty_bytes(terminal, b"output while pending".to_vec())
            .unwrap();
        let (pty_tx, pty_rx) = channels::unbounded();
        screen.bus.senders.to_pty = Some(SenderWithContext::new(pty_tx));
        let spawned = TerminalId::new(100);
        screen
            .land_layout(
                unfit_layout(),
                vec![],
                vec![(spawned, None)],
                vec![],
                vec![],
                HashMap::new(),
                pending_tab,
                true,
                client,
                None,
                None,
                &mut super::super::thread_main::PendingEvents::default(),
            )
            .unwrap();
        screen.settle_tabs().unwrap();
        let tab = screen.tabs.get(&pending_tab).expect(placement);
        assert!(!tab.is_pending(), "{placement}");
        assert!(tab
            .get_pane_with_id(pane_id)
            .unwrap()
            .dump_screen(false, None)
            .contains("output while pending"));
        assert_eq!(screen.get_active_tab(client).unwrap().id, active);
        let closed: Vec<_> = pty_rx
            .try_iter()
            .filter_map(|(instruction, _)| match instruction {
                PtyInstruction::ClosePane(id, _) => Some(id),
                PtyInstruction::CloseTab(ids) => {
                    assert!(!ids.contains(&pane_id), "{placement}");
                    None
                },
                _ => None,
            })
            .collect();
        assert_eq!(closed, vec![PaneId::Terminal(spawned)], "{placement}");
    }
}

#[test]
fn failed_override_preserves_all_existing_tabs_and_closes_only_spawned_panes() {
    let mut screen = screen_with_diverged_tab_ids();
    new_tab(&mut screen, 5, TabId::new(4));
    let before: Vec<_> = screen
        .tabs
        .iter()
        .map(|(id, tab)| (*id, tab.name.clone(), tab.get_all_pane_ids()))
        .collect();
    let (layouts, tabs_to_close) =
        dispatch_override(&mut screen, vec![layout_at(0, 1), layout_at(1, 2)], false);
    let (pty_tx, pty_rx) = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(pty_tx));
    let results = layouts
        .into_iter()
        .enumerate()
        .map(|(index, (id, layout))| crate::screen::TabOverrideResult {
            tab_index: id,
            tab_name: layout.tab_name,
            tiled_layout: if index == 1 {
                unfit_layout()
            } else {
                layout.tiled_layout
            },
            floating_layouts: vec![],
            swap_tiled_layouts: None,
            swap_floating_layouts: None,
            new_terminal_pids: vec![(TerminalId::new(100 + index as u32), None)],
            new_floating_pane_pids: vec![],
            plugin_ids: HashMap::new(),
        })
        .collect();
    assert!(screen
        .complete_override_layout(
            results,
            vec![],
            tabs_to_close,
            false,
            false,
            ClientId::new(1),
            &mut super::super::thread_main::PendingEvents::default(),
        )
        .is_err());
    let after: Vec<_> = screen
        .tabs
        .iter()
        .map(|(id, tab)| (*id, tab.name.clone(), tab.get_all_pane_ids()))
        .collect();
    assert_eq!(before, after);
    let closed: HashSet<_> = pty_rx
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PtyInstruction::ClosePane(id, _) => Some(id),
            _ => None,
        })
        .collect();
    assert_eq!(
        closed,
        HashSet::from([
            PaneId::Terminal(TerminalId::new(100)),
            PaneId::Terminal(TerminalId::new(101)),
        ])
    );
}

#[test]
fn failed_override_preserves_a_populated_pending_tab() {
    let mut screen = screen_with_diverged_tab_ids();
    let pane_id = PaneId::Terminal(TerminalId::new(50));
    add_pane_to_tab(&mut screen, TabId::new(2), pane_id, false);
    let tab_id = screen
        .break_multiple_panes_to_new_tab(vec![pane_id], None, false, None, ClientId::new(1))
        .unwrap();
    assert!(screen.tabs[&tab_id].is_pending());
    assert!(screen
        .complete_override_layout(
            vec![],
            vec![tab_id],
            vec![],
            false,
            false,
            ClientId::new(1),
            &mut super::super::thread_main::PendingEvents::default(),
        )
        .is_err());
    screen.settle_tabs().unwrap();
    assert!(screen.tabs[&tab_id].has_pane_with_pid(&pane_id));
    assert!(!screen.tabs[&tab_id].is_pending());
}

#[test]
fn layout_spawn_failure_preserves_moved_panes_and_releases_navigation() {
    for populated in [false, true] {
        let mut mock = MockScreen::new(Size { cols: 80, rows: 24 });
        let client = mock.main_client_id;
        let screen_thread = mock.run(None, vec![]);
        let pane_id = PaneId::Terminal(TerminalId::new(50));
        if populated {
            mock.to_screen
                .send(ScreenInstruction::NewPane(
                    pane_id,
                    None,
                    None,
                    None,
                    NewPanePlacement::default(),
                    false,
                    ClientTabIndexOrPaneId::TabIndex(TabId::new(0)),
                    None,
                    false,
                ))
                .unwrap();
        }
        mock.to_screen
            .send(ScreenInstruction::BreakPanesToNewTab {
                pane_ids: if populated { vec![pane_id] } else { vec![] },
                default_shell: None,
                should_change_focus_to_new_tab: false,
                new_tab_name: None,
                client_id: client,
                completion_tx: None,
            })
            .unwrap();
        let pending_tab = TabId::new(1);
        if populated {
            mock.to_screen
                .send(ScreenInstruction::PtyBytes(
                    TerminalId::new(50),
                    b"output while pending".to_vec(),
                ))
                .unwrap();
        }
        mock.to_screen
            .send(ScreenInstruction::GoToTab(
                TabNumber::new(2),
                Some(client),
                None,
            ))
            .unwrap();
        let focused_tab = || {
            let (tx, rx) = channels::bounded(1);
            mock.to_screen
                .send(ScreenInstruction::GetCurrentTabInfo {
                    client_id: client,
                    response_channel: tx,
                })
                .unwrap();
            rx.recv_timeout(yinz_test_support::default_timeout())
                .unwrap()
                .unwrap()
                .tab_id
        };
        // FIFO queries observe the queued switch before and after failure recovery.
        assert_eq!(focused_tab(), TabId::new(0));
        mock.to_screen
            .send(ScreenInstruction::LayoutSpawnFailed(pending_tab, None))
            .unwrap();
        assert_eq!(
            focused_tab(),
            if populated {
                pending_tab
            } else {
                TabId::new(0)
            }
        );
        let tab_info = || {
            let (tx, rx) = channels::bounded(1);
            mock.to_screen
                .send(ScreenInstruction::GetTabInfo {
                    tab_id: pending_tab,
                    response_channel: tx,
                })
                .unwrap();
            rx.recv_timeout(yinz_test_support::default_timeout())
                .unwrap()
        };
        assert_eq!(tab_info().is_some(), populated);
        assert!(!mock
            .pty_receiver
            .as_ref()
            .unwrap()
            .try_iter()
            .any(|(instruction, _)| match instruction {
                PtyInstruction::ClosePane(id, _) => id == pane_id,
                PtyInstruction::CloseTab(ids) => ids.contains(&pane_id),
                _ => false,
            }));
        if populated {
            let (tx, rx) = channels::bounded(1);
            mock.to_screen
                .send(ScreenInstruction::GetPaneScrollback {
                    pane_id,
                    client_id: client,
                    get_full_scrollback: false,
                    response_channel: tx,
                })
                .unwrap();
            let yinz_types::data::PaneScrollbackResponse::Ok(contents) = rx
                .recv_timeout(yinz_test_support::default_timeout())
                .unwrap()
            else {
                panic!("retained pane must remain readable");
            };
            assert!(contents
                .viewport
                .join("\n")
                .contains("output while pending"));
            mock.to_screen
                .send(ScreenInstruction::CloseTabWithId(pending_tab, None))
                .unwrap();
            assert!(tab_info().is_none(), "explicit close still removes the tab");
        }
        mock.teardown(vec![screen_thread]);
    }
}

#[test]
fn failed_new_layout_reports_error_preserves_focus_and_keeps_adopted_terminals() {
    for (adopted, held) in [(false, false), (true, false), (true, true)] {
        let mut screen = screen_with_diverged_tab_ids();
        let client_id = ClientId::new(1);
        let active = screen.get_active_tab(client_id).unwrap().id;
        let pending_tab = screen.get_new_tab_id();
        screen
            .new_tab(pending_tab, (vec![], vec![]), None, None)
            .unwrap();
        let (pty_tx, pty_rx) = channels::unbounded();
        screen.bus.senders.to_pty = Some(SenderWithContext::new(pty_tx));
        let (background_tx, background_rx) = channels::unbounded();
        screen.bus.senders.to_background_jobs = Some(SenderWithContext::new(background_tx));
        let (receipt_tx, receipt_rx) = channels::bounded(1);
        screen.pending_group_placements.insert(
            pending_tab,
            (
                yinz_types::data::WarpPlacement::NewTab,
                None,
                client_id,
                Some(receipt_tx),
                None,
            ),
        );
        let (completion_tx, completion_rx) = tokio::sync::oneshot::channel();
        let (blocking_tx, blocking_rx) = tokio::sync::oneshot::channel();
        let transferred = TerminalId::new(100);
        let spawned = TerminalId::new(101);
        let transferred_floating = TerminalId::new(102);
        let command = RunCommand {
            command: PathBuf::from("codex"),
            args: vec!["--resume".into()],
            cwd: Some(PathBuf::from("/work")),
            hold_on_close: true,
            hold_on_start: held,
            use_terminal_title: true,
            ..Default::default()
        };
        screen
            .land_layout(
                unfit_layout(),
                vec![FloatingPaneLayout::default()],
                vec![
                    (transferred, held.then(|| command.clone())),
                    (spawned, None),
                ],
                vec![(transferred_floating, held.then(|| command.clone()))],
                if adopted {
                    vec![
                        (transferred, command.clone(), None),
                        (transferred_floating, command.clone(), None),
                    ]
                } else {
                    vec![]
                },
                HashMap::new(),
                pending_tab,
                true,
                client_id,
                Some(crate::route::NotificationEnd::new(completion_tx)),
                Some((spawned, crate::route::NotificationEnd::new(blocking_tx))),
                &mut super::super::thread_main::PendingEvents::default(),
            )
            .unwrap();
        for result in [
            completion_rx.blocking_recv().unwrap(),
            blocking_rx.blocking_recv().unwrap(),
        ] {
            assert_eq!(result.exit_status, Some(1));
            assert!(result.error_message.is_some());
        }
        let active_pane = screen.get_active_pane_id(&client_id).unwrap();
        assert!(background_rx.try_iter().any(|(job, _)| matches!(job,
            BackgroundJob::DisplayPaneError(ids, message)
                if ids == vec![active_pane] && message.contains("Failed to apply layout"))));
        assert_eq!(receipt_rx.try_recv().unwrap().is_ok(), adopted);
        assert_eq!(screen.get_active_tab(client_id).unwrap().id, active);
        assert_eq!(screen.tabs.contains_key(&pending_tab), adopted);
        if adopted {
            let tab = &screen.tabs[&pending_tab];
            assert!(!tab.is_pending());
            for id in [transferred, transferred_floating] {
                let pane_id = PaneId::Terminal(id);
                let pane = tab.get_pane_with_id(pane_id).unwrap();
                assert_eq!(pane.invoked_with(), &Some(Run::Command(command.clone())));
                assert_eq!(pane.is_held(), held);
                let info = tab
                    .pane_infos()
                    .into_iter()
                    .find(|p| p.id == pane_id)
                    .unwrap();
                assert_eq!(info.is_suppressed, id == transferred_floating);
            }
        }
        let closed: HashSet<_> = pty_rx
            .try_iter()
            .filter_map(|(instruction, _)| match instruction {
                PtyInstruction::ClosePane(id, _) => Some(id),
                _ => None,
            })
            .collect();
        assert!(closed.contains(&PaneId::Terminal(spawned)));
        assert_eq!(closed.contains(&PaneId::Terminal(transferred)), !adopted);
        assert_eq!(
            closed.contains(&PaneId::Terminal(transferred_floating)),
            !adopted
        );
    }
}

#[test]
fn failed_hidden_warp_companion_keeps_source_focus_and_adopted_terminals() {
    use yinz_types::data::{WarpHiddenPane, WarpRelationships};

    let mut screen = screen_with_diverged_tab_ids();
    let client_id = ClientId::new(1);
    let source_tab = screen.get_active_tab(client_id).unwrap().id;
    let source_pane = screen.get_active_pane_id(&client_id);
    let source_panes = screen.tabs[&source_tab].get_all_pane_ids();
    let (pty_tx, pty_rx) = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(pty_tx));
    let (writer_tx, writer_rx) = channels::unbounded();
    drop(writer_rx);
    screen.bus.senders.to_pty_writer = Some(SenderWithContext::new(writer_tx));
    let pending_tab = screen.get_new_tab_id();
    screen
        .new_tab(pending_tab, (vec![], vec![]), None, None)
        .unwrap();
    screen
        .tabs
        .get_mut(&pending_tab)
        .unwrap()
        .senders
        .should_silently_fail = false;
    screen.pending_warp_relationships.insert(
        pending_tab,
        WarpRelationships {
            hidden: vec![WarpHiddenPane {
                pane: 1,
                restored_by: Some(0),
                is_scrollback_editor: false,
            }],
            stacks: vec![],
        },
    );
    let visible = TerminalId::new(100);
    let hidden = TerminalId::new(101);
    let (completion_tx, completion_rx) = tokio::sync::oneshot::channel();
    screen
        .land_layout(
            TiledPaneLayout::default(),
            vec![FloatingPaneLayout::default()],
            vec![(visible, None)],
            vec![(hidden, None)],
            vec![
                (visible, RunCommand::default(), None),
                (hidden, RunCommand::default(), None),
            ],
            HashMap::new(),
            pending_tab,
            true,
            client_id,
            Some(crate::route::NotificationEnd::new(completion_tx)),
            None,
            &mut super::super::thread_main::PendingEvents::default(),
        )
        .unwrap();

    let completion = completion_rx.blocking_recv().unwrap();
    assert_eq!(completion.exit_status, Some(1));
    let error = completion.error_message.unwrap();
    assert!(error.contains("failed to park new pane"), "{error}");
    assert!(
        error.contains("failed to send message to pty writer"),
        "{error}"
    );
    assert_eq!(screen.get_active_tab(client_id).unwrap().id, source_tab);
    assert_eq!(screen.get_active_pane_id(&client_id), source_pane);
    assert!(screen.tabs[&source_tab].has_connected_client(&client_id));
    assert_eq!(screen.tabs[&source_tab].get_all_pane_ids(), source_panes);
    for id in [visible, hidden] {
        assert!(screen.tabs[&pending_tab].has_pane_with_pid(&PaneId::Terminal(id)));
    }
    assert!(!screen.tabs[&pending_tab].is_pending());
    assert!(!pty_rx
        .try_iter()
        .any(|(instruction, _)| matches!(instruction, PtyInstruction::ClosePane(_, _))));
}

#[test]
fn reporting_failure_after_layout_commit_keeps_the_new_panes() {
    let mut screen = screen_with_diverged_tab_ids();
    let tab_id = screen.get_new_tab_id();
    screen
        .new_tab(tab_id, (vec![], vec![]), None, None)
        .unwrap();
    let (server_tx, server_rx) = channels::unbounded();
    drop(server_rx);
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_tx));
    screen.bus.senders.should_silently_fail = false;
    let (pty_tx, pty_rx) = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(pty_tx));
    let terminal_id = TerminalId::new(100);
    screen
        .land_layout(
            TiledPaneLayout::default(),
            vec![],
            vec![(terminal_id, None)],
            vec![],
            vec![],
            HashMap::new(),
            tab_id,
            false,
            ClientId::new(1),
            None,
            None,
            &mut super::super::thread_main::PendingEvents::default(),
        )
        .unwrap();
    assert!(screen.tabs[&tab_id].has_pane_with_pid(&PaneId::Terminal(terminal_id)));
    assert!(!screen.tabs[&tab_id].is_pending());
    assert!(!pty_rx.try_iter().any(|(instruction, _)| matches!(
        instruction, PtyInstruction::ClosePane(PaneId::Terminal(id), _) if id == terminal_id
    )));
}
