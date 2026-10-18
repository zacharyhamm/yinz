use super::*;
use crate::PluginId;
use yinz_types::internal_ids::TabNumber;
use yinz_types::{input::actions::Action, tab_bar::TabGroupInfo};

const CLIENT: ClientId = ClientId::new(1);

fn grouped_screen() -> Screen {
    let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 24 });
    for (id, group) in [(0, 0), (1, 1), (2, 0), (3, 1)] {
        let tab = TabId::new(id);
        new_tab(&mut screen, id as u32 + 1, tab);
        screen.restore_tab_group(
            tab,
            &TiledPaneLayout {
                tab_group: Some(TabGroupInfo {
                    id: group,
                    name: format!("Group {group}"),
                }),
                group_focused: true,
                ..Default::default()
            },
        );
    }
    screen
}

#[test]
fn group_names_follow_distinct_projects_in_tab_order() {
    let mut screen = grouped_screen();
    screen.pane_projects = [(1, "x"), (3, "y"), (2, "z"), (4, "z")]
        .into_iter()
        .map(|(id, name)| (PaneId::Terminal(TerminalId::new(id)), name.to_owned()))
        .collect();
    screen
        .get_tab_by_id_mut(TabId::new(0))
        .unwrap()
        .rename_by_user("notes".into());
    assert_eq!(screen.tab_group_info(TabId::new(0)).unwrap().name, "x + y");
    assert_eq!(screen.tab_group_name(1).unwrap(), "z");
    screen.move_group_tab(TabId::new(0), true).unwrap();
    assert_eq!(screen.tab_group_name(0).unwrap(), "y + x");
    screen.close_tab_by_id(TabId::new(2)).unwrap();
    assert_eq!(screen.tab_group_name(0).unwrap(), "x");
    screen
        .pane_projects
        .remove(&PaneId::Terminal(TerminalId::new(1)));
    assert_eq!(screen.tab_group_name(0).unwrap(), "Group 0");
}

#[test]
fn default_group_uses_projects_and_falls_back_when_none_are_known() {
    let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 24 });
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    assert_eq!(screen.tab_group_name(0).unwrap(), "Default");
    screen
        .pane_projects
        .insert(PaneId::Terminal(TerminalId::new(1)), "x".into());
    screen
        .pane_projects
        .insert(PaneId::Terminal(TerminalId::new(99)), "unrelated".into());
    assert_eq!(screen.tab_group_name(0).unwrap(), "x");
    screen
        .pane_projects
        .insert(PaneId::Terminal(TerminalId::new(1)), "\n\r".into());
    assert_eq!(screen.tab_group_name(0).unwrap(), "Default");
}

#[test]
fn group_names_include_each_project_within_a_tab_without_branch_labels() {
    let mut screen = grouped_screen();
    screen
        .get_tab_by_id_mut(TabId::new(0))
        .unwrap()
        .horizontal_split(
            PaneId::Terminal(TerminalId::new(9)),
            None,
            None,
            CLIENT,
            None,
            None,
        )
        .unwrap();
    screen
        .pane_projects
        .insert(PaneId::Terminal(TerminalId::new(1)), "x".into());
    screen
        .pane_projects
        .insert(PaneId::Terminal(TerminalId::new(9)), "y".into());
    screen
        .tab_projects
        .insert(TabId::new(0), "x:main + y:topic".into());
    assert_eq!(screen.tab_group_name(0).unwrap(), "x + y");
}

#[test]
fn group_navigation_uses_local_order_and_remembers_tabs() {
    let mut screen = grouped_screen();
    screen.switch_tab_next(None, true, CLIENT).unwrap();
    assert_eq!(screen.active_tab_ids[&CLIENT], TabId::new(1));
    screen.switch_tab_prev(None, true, CLIENT).unwrap();
    assert_eq!(screen.active_tab_ids[&CLIENT], TabId::new(3));
    screen
        .tab_group_action(
            Action::SwitchTabGroup { id: 0 },
            CLIENT,
            None,
            &mut HashSet::new(),
        )
        .unwrap();
    assert_eq!(screen.active_tab_ids[&CLIENT], TabId::new(2));
    screen
        .tab_group_action(
            Action::GoToTabInGroup {
                index: TabNumber::new(1),
            },
            CLIENT,
            None,
            &mut HashSet::new(),
        )
        .unwrap();
    assert_eq!(screen.active_tab_ids[&CLIENT], TabId::new(0));
    screen.toggle_tab(CLIENT).unwrap();
    assert_eq!(screen.active_tab_ids[&CLIENT], TabId::new(2));
    screen
        .tab_group_action(Action::GoToNextTabGroup, CLIENT, None, &mut HashSet::new())
        .unwrap();
    assert_eq!(screen.active_tab_ids[&CLIENT], TabId::new(3));
    screen.go_to_tab(TabNumber::new(1), CLIENT).unwrap();
    assert_eq!(
        screen.group_for_client(Some(CLIENT)),
        0,
        "explicit positions remain session-wide"
    );
}

#[test]
fn group_navigation_defers_stable_targets_until_tabs_load() {
    for closed in [None, Some(TabId::new(1)), Some(TabId::new(4))] {
        let mut screen = grouped_screen();
        let target = TabId::new(4);
        screen
            .new_tab(target, (vec![], vec![]), None, Some(CLIENT))
            .unwrap();
        let mut switches = HashSet::new();
        screen
            .tab_group_action(
                Action::GoToTabInGroup {
                    index: TabNumber::new(3),
                },
                CLIENT,
                None,
                &mut switches,
            )
            .unwrap();
        assert_eq!(screen.active_tab_ids[&CLIENT], TabId::new(3));
        assert_eq!(switches, HashSet::from([(target, CLIENT)]));
        if let Some(closed) = closed {
            screen.close_tab_by_id(closed).unwrap();
        }
        if closed != Some(target) {
            screen
                .apply_layout(
                    TiledPaneLayout::default(),
                    vec![],
                    vec![(TerminalId::new(9), None)],
                    vec![],
                    HashMap::new(),
                    target,
                    false,
                    CLIENT,
                    None,
                )
                .unwrap();
            assert!(!screen.tabs[&target].is_pending());
            if closed.is_some() {
                assert_eq!(screen.tabs[&target].position, TabPosition::new(3));
            }
        }
        assert!(!screen.has_pending_tabs());
        screen.apply_deferred_tab_switches(&mut switches).unwrap();
        assert!(switches.is_empty());
        assert_eq!(
            screen.active_tab_ids[&CLIENT],
            if closed == Some(target) {
                TabId::new(3)
            } else {
                target
            }
        );
    }
}

#[test]
fn group_navigation_to_loaded_tab_waits_for_other_pending_tabs() {
    let mut screen = grouped_screen();
    screen
        .new_tab(TabId::new(4), (vec![], vec![]), None, Some(CLIENT))
        .unwrap();
    let mut switches = HashSet::new();
    screen
        .tab_group_action(
            Action::GoToTabInGroup {
                index: TabNumber::new(1),
            },
            CLIENT,
            None,
            &mut switches,
        )
        .unwrap();
    assert_eq!(screen.active_tab_ids[&CLIENT], TabId::new(3));
    assert_eq!(switches, HashSet::from([(TabId::new(1), CLIENT)]));
}

#[test]
fn new_group_tab_captures_membership_before_layout_arrives() {
    let mut screen = grouped_screen();
    let (tx, rx): ChannelWithContext<PluginInstruction> = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(tx));
    screen
        .tab_group_action(Action::NewTabGroup, CLIENT, None, &mut HashSet::new())
        .unwrap();
    let tab = screen.tabs.keys().next_back().copied().unwrap();
    let group = screen.tab_group_ids[&tab];
    assert_eq!(screen.tab_groups[&group].name, "Group 2");
    assert!(screen.tabs[&tab].is_pending());
    assert!(rx
        .try_iter()
        .any(|(i, _)| matches!(i, PluginInstruction::NewTab(..))));
    screen
        .tab_group_action(
            Action::SwitchTabGroup { id: 0 },
            CLIENT,
            None,
            &mut HashSet::new(),
        )
        .unwrap();
    screen
        .apply_layout(
            TiledPaneLayout::default(),
            vec![],
            vec![(TerminalId::new(9), None)],
            vec![],
            HashMap::new(),
            tab,
            true,
            CLIENT,
            None,
        )
        .unwrap();
    assert_eq!(screen.tab_group_ids[&tab], group);
    let next = screen.get_new_tab_id();
    new_tab(&mut screen, 10, next);
    assert_eq!(screen.tab_group_ids[&next], group);
}

#[test]
fn closing_tabs_removes_only_empty_groups_and_relocates_focus() {
    let mut screen = grouped_screen();
    screen.close_tab_by_id(TabId::new(3)).unwrap();
    assert_eq!(screen.active_tab_ids[&CLIENT], TabId::new(1));
    assert_eq!(screen.tab_groups.len(), 2);
    screen.close_tab_by_id(TabId::new(1)).unwrap();
    assert_eq!(screen.group_for_client(Some(CLIENT)), 0);
    assert_eq!(screen.tab_groups.len(), 1);
    assert!(!screen.tab_groups.contains_key(&1));
    assert!(screen
        .tab_group_action(
            Action::RenameTabGroup {
                id: 1,
                name: "Gone".into()
            },
            CLIENT,
            None,
            &mut HashSet::new(),
        )
        .is_err());
}

#[test]
fn closing_group_syncs_scroll_mode_for_every_relocated_client() {
    for destination_scrolled in [false, true] {
        let mut screen = grouped_screen();
        let other = ClientId::new(2);
        screen.add_client(other).unwrap();
        screen.go_to_tab(TabNumber::new(2), other).unwrap();
        for id in if destination_scrolled { [0, 2] } else { [1, 3] } {
            let pane = screen
                .tabs
                .get_mut(&TabId::new(id))
                .unwrap()
                .get_pane_with_id_mut(PaneId::Terminal(TerminalId::new(id as u32 + 1)))
                .unwrap();
            pane.handle_pty_bytes(b"scrollback\r\n".repeat(100));
            pane.scroll_up(5, Some(CLIENT));
        }
        let (before, after) = if destination_scrolled {
            (InputMode::Normal, InputMode::Scroll)
        } else {
            (InputMode::Scroll, InputMode::Normal)
        };
        for client in [CLIENT, other] {
            screen.change_mode(before, None, client).unwrap();
            assert_eq!(
                screen.active_pane_is_scrolled(client),
                !destination_scrolled
            );
        }
        let receiver = screen_wired_to_server(&mut screen);
        screen
            .tab_group_action(
                Action::CloseTabGroup { id: 1 },
                CLIENT,
                None,
                &mut HashSet::new(),
            )
            .unwrap();
        let modes: HashMap<_, _> = receiver
            .try_iter()
            .filter_map(|(instruction, _)| match instruction {
                ServerInstruction::ChangeMode(client, mode, _) => Some((client, mode)),
                _ => None,
            })
            .collect();
        assert_eq!(modes, HashMap::from([(CLIENT, after), (other, after)]));
        for client in [CLIENT, other] {
            assert_eq!(screen.group_for_client(Some(client)), 0);
            assert_eq!(screen.active_pane_is_scrolled(client), destination_scrolled);
        }
    }
}

#[test]
fn closing_tab_skips_unusable_group_history_and_preferences() {
    for pending in [false, true] {
        for remembered_by in ["history", "preferred", "position"] {
            for fallback_group in [false, true] {
                let mut screen = grouped_screen();
                let source = TabId::new(4);
                new_tab(&mut screen, 5, source);
                if fallback_group {
                    screen.restore_tab_group(
                        source,
                        &TiledPaneLayout {
                            tab_group: Some(TabGroupInfo {
                                id: 2,
                                name: "Closing".into(),
                            }),
                            ..Default::default()
                        },
                    );
                }
                let unusable = TabId::new(1);
                screen
                    .tabs
                    .get_mut(&unusable)
                    .unwrap()
                    .suppress_pane(PaneId::Terminal(TerminalId::new(2)), None);
                if pending {
                    screen.close_tab_by_id(unusable).unwrap();
                    screen
                        .new_tab(unusable, (vec![], vec![]), None, Some(CLIENT))
                        .unwrap();
                    screen.tab_group_ids.insert(unusable, 1);
                    // Keep the unusable tab first in group order even after recreating it.
                    screen.tabs.get_mut(&unusable).unwrap().position = TabPosition::new(1);
                    screen.tabs.get_mut(&TabId::new(2)).unwrap().position = TabPosition::new(2);
                    screen.tabs.get_mut(&TabId::new(3)).unwrap().position = TabPosition::new(3);
                    screen.tabs.get_mut(&source).unwrap().position = TabPosition::new(4);
                }
                screen.tab_history.insert(
                    CLIENT,
                    if remembered_by == "history" {
                        vec![unusable]
                    } else {
                        vec![]
                    },
                );
                screen.tab_groups.get_mut(&1).unwrap().preferred =
                    (remembered_by == "preferred").then_some(unusable);
                screen.close_tab_by_id(source).unwrap();
                assert_eq!(
                    screen.active_tab_ids[&CLIENT],
                    TabId::new(3),
                    "pending={pending}, remembered_by={remembered_by}, fallback={fallback_group}"
                );
                assert_eq!(
                    screen.get_active_pane_id(&CLIENT),
                    Some(PaneId::Terminal(TerminalId::new(4)))
                );
            }
        }
    }
}

#[test]
fn group_navigation_recovers_hidden_preferred_terminal() {
    for action in [Action::SwitchTabGroup { id: 0 }, Action::GoToNextTabGroup] {
        let mut screen = grouped_screen();
        let hidden = PaneId::Terminal(TerminalId::new(3));
        screen
            .tabs
            .get_mut(&TabId::new(2))
            .unwrap()
            .suppress_pane(hidden, None);
        assert!(!screen.tabs[&TabId::new(2)].has_selectable_panes());
        let metadata = screen.get_layout_metadata(None, None);
        let (raw, _) =
            yinz_session::session_serialization::serialize_session_layout(metadata.into()).unwrap();
        let layout = yinz_config::kdl::layout::from_kdl(&raw, None, None, None).unwrap();
        assert!(layout.tabs[2].1.group_focused);
        screen
            .tab_group_action(action, CLIENT, None, &mut HashSet::new())
            .unwrap();
        assert_eq!(screen.active_tab_ids[&CLIENT], TabId::new(2));
        assert_eq!(screen.get_active_pane_id(&CLIENT), Some(hidden));
        assert!(screen.tabs[&TabId::new(2)].has_selectable_panes());
    }
}

#[test]
fn palette_moves_stay_in_source_group_and_wrap_in_both_directions() {
    for direction in [Direction::Left, Direction::Right] {
        for source in [0, 2, 4] {
            let mut screen = grouped_screen();
            screen.go_to_tab(TabNumber::new(1), CLIENT).unwrap();
            new_tab(&mut screen, 5, TabId::new(4));
            let source = TabId::new(source);
            let destination = match (source.get(), direction) {
                (0, Direction::Left) | (2, Direction::Right) => TabId::new(4),
                (0, Direction::Right) | (4, Direction::Left) => TabId::new(2),
                _ => TabId::new(0),
            };
            let pane = PaneId::Terminal(TerminalId::new(source.get() as u32 + 1));
            // Palette actions retain their source even if the client has since changed groups.
            screen.go_to_tab(TabNumber::new(2), CLIENT).unwrap();
            screen
                .move_command_palette_pane(pane, source, Some(direction), CLIENT)
                .unwrap();
            assert_eq!(screen.active_tab_ids[&CLIENT], destination);
            assert_eq!(screen.tab_group_ids[&destination], 0);
            assert!(screen.tabs[&destination].has_pane_with_pid(&pane));
            assert_eq!(screen.get_active_pane_id(&CLIENT), Some(pane));
        }
    }
}

#[test]
fn palette_move_from_singleton_group_reports_no_other_tab() {
    for direction in [Direction::Left, Direction::Right] {
        let mut screen = grouped_screen();
        screen.close_tab_by_id(TabId::new(1)).unwrap();
        let source = TabId::new(3);
        let pane = PaneId::Terminal(TerminalId::new(4));
        let (tx, rx) = channels::unbounded();
        screen.bus.senders.to_screen = Some(SenderWithContext::new(tx));
        screen
            .move_command_palette_pane(pane, source, Some(direction), CLIENT)
            .unwrap();
        assert_eq!(screen.tabs.len(), 3);
        assert_eq!(screen.active_tab_ids[&CLIENT], source);
        assert_eq!(screen.get_active_pane_id(&CLIENT), Some(pane));
        assert!(rx.try_iter().any(|(instruction, _)| matches!(
            instruction,
            ScreenInstruction::Notify(message, ..) if message == "Pane was not moved: no other tab"
        )));
    }
}

#[test]
fn closing_a_tab_keeps_group_history_ready_for_toggle() {
    let mut screen = grouped_screen();
    new_tab(&mut screen, 5, TabId::new(4));
    screen.close_tab_by_id(TabId::new(4)).unwrap();
    assert_eq!(screen.active_tab_ids[&CLIENT], TabId::new(3));
    assert_eq!(screen.tab_groups[&1].preferred, Some(TabId::new(3)));
    assert!(!screen.tab_history[&CLIENT].contains(&TabId::new(4)));
    screen.toggle_tab(CLIENT).unwrap();
    assert_eq!(screen.active_tab_ids[&CLIENT], TabId::new(1));
}

#[test]
fn group_close_and_reorder_preserve_other_groups() {
    let mut screen = grouped_screen();
    screen.move_active_tab_to_left(CLIENT).unwrap();
    assert_eq!(screen.tabs[&TabId::new(0)].position, TabPosition::new(0));
    assert_eq!(screen.tabs[&TabId::new(2)].position, TabPosition::new(2));
    assert_eq!(screen.tabs[&TabId::new(3)].position, TabPosition::new(1));
    screen
        .tab_group_action(
            Action::CloseTabGroup { id: 1 },
            CLIENT,
            None,
            &mut HashSet::new(),
        )
        .unwrap();
    assert_eq!(screen.tabs.len(), 2);
    assert!(screen.tabs.contains_key(&TabId::new(0)));
    assert!(screen.tabs.contains_key(&TabId::new(2)));
}

#[test]
fn closing_last_group_confirms_before_removing_its_guarding_plugin() {
    for destination in ["none", "pending", "nonselectable", "loaded"] {
        let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 24 });
        let (plugin_tx, _plugin_rx) = channels::unbounded();
        screen.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
        new_tab(&mut screen, 1, TabId::new(0));
        new_tab(&mut screen, 2, TabId::new(1));
        let plugin = PluginId::new(42);
        add_pane_to_tab(&mut screen, TabId::new(0), PaneId::Plugin(plugin), false);
        screen.set_quit_notice(plugin, Some("agent is still working".into()), true);
        if destination != "none" {
            let other = TabId::new(2);
            if destination == "pending" {
                screen.new_tab(other, (vec![], vec![]), None, None).unwrap();
            } else {
                new_tab(&mut screen, 3, other);
                if destination == "nonselectable" {
                    screen
                        .tabs
                        .get_mut(&other)
                        .unwrap()
                        .suppress_pane(PaneId::Terminal(TerminalId::new(3)), None);
                }
            }
            screen.restore_tab_group(
                other,
                &TiledPaneLayout {
                    tab_group: Some(TabGroupInfo {
                        id: 1,
                        name: "Other".into(),
                    }),
                    ..Default::default()
                },
            );
        }
        let acting_client = ClientId::new(2);
        screen.add_client(acting_client).unwrap();
        let before = screen.tab_group_ids.clone();
        let (tx, rx) = channels::unbounded();
        screen.bus.senders.to_screen = Some(SenderWithContext::new(tx));
        let (completion_tx, mut completion_rx) = tokio::sync::oneshot::channel();
        screen
            .tab_group_action(
                Action::CloseTabGroup { id: 0 },
                acting_client,
                Some(crate::NotificationEnd::new(completion_tx)),
                &mut HashSet::new(),
            )
            .unwrap();
        let launch = rx
            .try_iter()
            .find_map(|(instruction, _)| match instruction {
                ScreenInstruction::LaunchPlugin(
                    plugin,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    client,
                    completion,
                    ..,
                ) => Some((plugin, client, completion)),
                _ => None,
            });
        if destination == "loaded" {
            assert!(launch.is_none());
            assert_eq!(screen.tabs.len(), 1);
            assert!(screen.tabs.contains_key(&TabId::new(2)));
            assert!(!screen.quit_is_guarded());
        } else {
            let (plugin, client, completion) = launch.expect(destination);
            assert_eq!(client, acting_client);
            let RunPluginOrAlias::RunPlugin(plugin) = plugin else {
                panic!("expected quit confirmation plugin");
            };
            assert_eq!(
                plugin.configuration.inner()["notice"],
                "agent is still working"
            );
            assert_eq!(plugin.configuration.inner()["reason"], "last_pane");
            assert!(completion.is_some());
            assert!(completion_rx.try_recv().is_err());
            drop(completion);
            assert!(completion_rx.try_recv().is_ok());
            assert_eq!(screen.tab_group_ids, before, "{destination}");
            assert!(
                screen.tabs[&TabId::new(0)].has_pane_with_pid(&PaneId::Plugin(PluginId::new(42)))
            );
            assert!(screen.quit_is_guarded());
            assert!(screen.deferred_tab_closes.is_empty());
        }
    }
}

#[test]
fn closing_last_unguarded_group_closes_every_tab() {
    let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 24 });
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    screen
        .tab_group_action(
            Action::CloseTabGroup { id: 0 },
            CLIENT,
            None,
            &mut HashSet::new(),
        )
        .unwrap();
    assert!(screen.tabs.is_empty());
    assert!(screen.tab_groups.is_empty());
}

#[test]
fn group_focus_is_per_client() {
    let mut screen = grouped_screen();
    let other = ClientId::new(2);
    screen.add_client(other).unwrap();
    screen.go_to_tab(TabNumber::new(1), other).unwrap();
    assert_eq!(screen.group_for_client(Some(other)), 0);
    assert_eq!(screen.group_for_client(Some(CLIENT)), 1);
    screen
        .tab_group_action(
            Action::SwitchTabGroup { id: 0 },
            CLIENT,
            None,
            &mut HashSet::new(),
        )
        .unwrap();
    assert_eq!(screen.active_tab_ids[&CLIENT], TabId::new(2));
    assert_eq!(screen.active_tab_ids[&other], TabId::new(0));
}

#[test]
fn saved_groups_round_trip_names_order_membership_and_preferred_tabs() {
    let mut screen = grouped_screen();
    screen
        .tab_group_action(
            Action::RenameTabGroup {
                id: 0,
                name: "工程".into(),
            },
            CLIENT,
            None,
            &mut HashSet::new(),
        )
        .unwrap();
    screen
        .pane_projects
        .insert(PaneId::Terminal(TerminalId::new(1)), "project".into());
    assert_eq!(screen.tab_group_name(0).unwrap(), "project");
    let metadata = screen.get_layout_metadata(None, None);
    let (raw, _) =
        yinz_session::session_serialization::serialize_session_layout(metadata.into()).unwrap();
    let layout = yinz_config::kdl::layout::from_kdl(&raw, None, None, None).unwrap();
    assert_eq!(
        layout
            .tabs
            .iter()
            .map(|(_, layout, _)| layout.tab_group.as_ref().unwrap().id)
            .collect::<Vec<_>>(),
        vec![0, 1, 0, 1]
    );
    assert_eq!(layout.tabs[0].1.tab_group.as_ref().unwrap().name, "工程");
    assert!(layout.tabs[2].1.group_focused);
    assert!(layout.tabs[3].1.group_focused);
    assert!(!layout.tabs[0].1.group_focused);
    let mut restored = create_non_mirrored_screen(Size { cols: 80, rows: 24 });
    for (index, (_, layout, _)) in layout.tabs.iter().enumerate() {
        let id = TabId::new(index);
        restored
            .new_tab(id, (vec![], vec![]), None, Some(CLIENT))
            .unwrap();
        restored.restore_tab_group(id, layout);
    }
    assert_eq!(restored.tab_groups[&0].name, "工程");
    assert_eq!(restored.tab_groups[&1].preferred, Some(TabId::new(3)));
    assert_eq!(restored.next_tab_group_id, 2);
    for (index, (_, layout, _)) in layout.tabs.into_iter().enumerate() {
        restored
            .apply_layout(
                layout,
                vec![],
                vec![(TerminalId::new(index as u32 + 10), None)],
                vec![],
                HashMap::new(),
                TabId::new(index),
                index == 3,
                CLIENT,
                None,
            )
            .unwrap();
    }
    restored
        .tab_group_action(
            Action::SwitchTabGroup { id: 0 },
            CLIENT,
            None,
            &mut HashSet::new(),
        )
        .unwrap();
    assert_eq!(restored.active_tab_ids[&CLIENT], TabId::new(2));
}

#[test]
fn last_pane_exit_removes_group_but_pending_tabs_keep_it_alive() {
    let mut screen = grouped_screen();
    for id in [1, 3] {
        screen.tabs.get_mut(&TabId::new(id)).unwrap().close_pane(
            PaneId::Terminal(TerminalId::new(id as u32 + 1)),
            false,
            None,
        );
    }
    screen.settle_tabs().unwrap();
    assert!(!screen.tab_groups.contains_key(&1));
    let pending = screen.get_new_tab_id();
    screen
        .new_tab(pending, (vec![], vec![]), None, Some(CLIENT))
        .unwrap();
    screen.close_tab_by_id(TabId::new(0)).unwrap();
    screen.close_tab_by_id(TabId::new(2)).unwrap();
    assert!(screen.tab_groups.contains_key(&0));
    assert!(screen.tabs[&pending].is_pending());
}

#[test]
fn move_pane_between_groups_follows_both_directions_and_wraps() {
    for (action, source, destination) in [
        (Action::MovePaneToNextTabGroup, 0, 1),
        (Action::MovePaneToPreviousTabGroup, 0, 3),
        (Action::MovePaneToNextTabGroup, 3, 2),
        (Action::MovePaneToPreviousTabGroup, 1, 2),
    ] {
        for floating in [false, true] {
            let mut screen = grouped_screen();
            screen.restore_tab_group(
                TabId::new(3),
                &TiledPaneLayout {
                    tab_group: Some(TabGroupInfo {
                        id: 2,
                        name: "Third".into(),
                    }),
                    ..Default::default()
                },
            );
            screen
                .switch_active_tab(TabPosition::new(source), None, true, CLIENT)
                .unwrap();
            let pane = PaneId::Terminal(TerminalId::new(42));
            add_pane_to_tab(&mut screen, TabId::new(source), pane, floating);
            screen
                .get_active_tab_mut(CLIENT)
                .unwrap()
                .focus_pane_with_id(pane, false, false, CLIENT)
                .unwrap();
            assert_eq!(screen.get_active_pane_id(&CLIENT), Some(pane));
            screen
                .tab_group_action(action.clone(), CLIENT, None, &mut HashSet::new())
                .unwrap();
            assert_eq!(screen.active_tab_ids[&CLIENT], TabId::new(destination));
            assert_eq!(screen.get_active_pane_id(&CLIENT), Some(pane));
            assert_eq!(
                screen.tabs[&TabId::new(destination)].pane_id_is_floating(&pane),
                floating
            );
            assert!(!screen.tabs[&TabId::new(source)].has_pane_with_pid(&pane));
        }
    }
}

#[test]
fn move_pane_between_groups_syncs_scroll_mode_with_final_focus() {
    for action in [
        Action::MovePaneToNextTabGroup,
        Action::MovePaneToPreviousTabGroup,
    ] {
        for source_scrolled in [false, true] {
            for mode in [InputMode::Normal, InputMode::Scroll, InputMode::Tab] {
                let mut screen = grouped_screen();
                let source = TabId::new(3);
                let destination = TabId::new(2);
                let moved = PaneId::Terminal(TerminalId::new(4));
                let resident = PaneId::Terminal(TerminalId::new(3));
                for (tab, pane, scrolled) in [
                    (source, moved, source_scrolled),
                    (destination, resident, !source_scrolled),
                ] {
                    let pane = screen
                        .tabs
                        .get_mut(&tab)
                        .unwrap()
                        .get_pane_with_id_mut(pane)
                        .unwrap();
                    pane.handle_pty_bytes(b"scrollback\r\n".repeat(100));
                    if scrolled {
                        pane.scroll_up(5, Some(CLIENT));
                    }
                }
                screen.change_mode(mode, None, CLIENT).unwrap();
                assert_eq!(screen.get_active_pane_id(&CLIENT), Some(moved));
                let receiver = screen_wired_to_server(&mut screen);
                screen
                    .tab_group_action(action.clone(), CLIENT, None, &mut HashSet::new())
                    .unwrap();

                assert_eq!(screen.active_tab_ids[&CLIENT], destination);
                assert_eq!(screen.get_active_pane_id(&CLIENT), Some(moved));
                for (pane, scrolled) in [(moved, source_scrolled), (resident, !source_scrolled)] {
                    assert_eq!(
                        screen.tabs[&destination]
                            .get_pane_with_id(pane)
                            .unwrap()
                            .scroll_position()
                            .0,
                        if scrolled { 5 } else { 0 },
                    );
                }
                let expected_mode = if mode == InputMode::Tab {
                    mode
                } else if source_scrolled {
                    InputMode::Scroll
                } else {
                    InputMode::Normal
                };
                let changes: Vec<_> = receiver
                    .try_iter()
                    .filter_map(|(instruction, _)| match instruction {
                        ServerInstruction::ChangeMode(client, mode, _) => Some((client, mode)),
                        _ => None,
                    })
                    .collect();
                assert_eq!(
                    changes,
                    if mode == expected_mode {
                        vec![]
                    } else {
                        vec![(CLIENT, expected_mode)]
                    },
                    "action {action:?}, source scrolled {source_scrolled}, initial mode {mode:?}",
                );
                // Apply the queued server response only after checking for intermediate-focus changes.
                for (client, mode) in changes {
                    screen.change_mode(mode, None, client).unwrap();
                }
                assert_eq!(screen.mode_info[&CLIENT].mode, expected_mode);
                assert_eq!(screen.active_pane_is_scrolled(CLIENT), source_scrolled);
            }
        }
    }
}

#[test]
fn move_pane_to_group_uses_history_then_preferred_then_first_loaded_tab() {
    for (history, preferred, expected) in [(true, true, 0), (false, true, 2), (false, false, 0)] {
        let mut screen = grouped_screen();
        screen
            .tab_history
            .insert(CLIENT, if history { vec![TabId::new(0)] } else { vec![] });
        screen.tab_groups.get_mut(&0).unwrap().preferred = preferred.then_some(TabId::new(2));
        let pane = screen.get_active_pane_id(&CLIENT).unwrap();
        screen
            .tab_group_action(
                Action::MovePaneToNextTabGroup,
                CLIENT,
                None,
                &mut HashSet::new(),
            )
            .unwrap();
        assert_eq!(screen.active_tab_ids[&CLIENT], TabId::new(expected));
        assert_eq!(screen.get_active_pane_id(&CLIENT), Some(pane));
        screen.settle_tabs().unwrap();
        assert!(
            !screen.tabs.contains_key(&TabId::new(3)),
            "empty source tab is cleaned up"
        );
        assert_eq!(screen.tabs.len(), 3, "no replacement tab");
    }
}

#[test]
fn move_pane_to_group_skips_pending_groups() {
    let mut screen = grouped_screen();
    screen.restore_tab_group(
        TabId::new(3),
        &TiledPaneLayout {
            tab_group: Some(TabGroupInfo {
                id: 2,
                name: "Third".into(),
            }),
            ..Default::default()
        },
    );
    screen.close_tab_by_id(TabId::new(1)).unwrap();
    screen
        .new_tab(TabId::new(4), (vec![], vec![]), None, Some(CLIENT))
        .unwrap();
    screen.restore_tab_group(
        TabId::new(4),
        &TiledPaneLayout {
            tab_group: Some(TabGroupInfo {
                id: 1,
                name: "Pending".into(),
            }),
            ..Default::default()
        },
    );
    screen
        .switch_active_tab(TabPosition::new(0), None, true, CLIENT)
        .unwrap();
    let pane = screen.get_active_pane_id(&CLIENT).unwrap();
    screen
        .tab_group_action(
            Action::MovePaneToNextTabGroup,
            CLIENT,
            None,
            &mut HashSet::new(),
        )
        .unwrap();
    assert_eq!(screen.active_tab_ids[&CLIENT], TabId::new(3));
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(pane));
}

#[test]
fn move_pane_to_group_without_destination_or_room_preserves_pane_and_focus() {
    for no_other_group in [true, false] {
        let mut screen = grouped_screen();
        let source = TabId::new(3);
        if no_other_group {
            for group in screen.tab_group_ids.values_mut() {
                *group = 1;
            }
            screen.prune_tab_groups();
        } else {
            screen
                .tabs
                .get_mut(&TabId::new(2))
                .unwrap()
                .resize_whole_tab(Size { cols: 4, rows: 2 })
                .unwrap();
        }
        let pane = screen.get_active_pane_id(&CLIENT).unwrap();
        let (sender, errors) = channels::unbounded();
        screen.bus.senders.to_background_jobs = Some(SenderWithContext::new(sender));
        screen
            .tab_group_action(
                Action::MovePaneToNextTabGroup,
                CLIENT,
                None,
                &mut HashSet::new(),
            )
            .unwrap();
        assert!(errors.try_iter().any(|(job, _)| matches!(job,
            BackgroundJob::DisplayPaneError(panes, message)
                if panes == vec![pane] && message == if no_other_group {
                    "No other tab groups to add pane to!"
                } else { "Could not move pane to tab group!" }
        )));
        assert_eq!(screen.active_tab_ids[&CLIENT], source);
        assert_eq!(screen.get_active_pane_id(&CLIENT), Some(pane));
        assert!(screen.tabs[&source].has_pane_with_pid(&pane));
        assert_eq!(screen.tabs.len(), 4);
    }
}

#[test]
fn moving_last_pane_out_of_group_removes_group_without_replacement() {
    let mut screen = grouped_screen();
    screen.close_tab_by_id(TabId::new(1)).unwrap();
    let pane = screen.get_active_pane_id(&CLIENT).unwrap();
    screen
        .tab_group_action(
            Action::MovePaneToPreviousTabGroup,
            CLIENT,
            None,
            &mut HashSet::new(),
        )
        .unwrap();
    screen.settle_tabs().unwrap();
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(pane));
    assert_eq!(screen.tabs.len(), 2);
    assert_eq!(screen.tab_groups.len(), 1);
    assert!(screen.tab_groups.contains_key(&0));
}
