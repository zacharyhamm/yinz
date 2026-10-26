use super::*;
use yinz_bus::plugins::PluginInstruction;
use yinz_bus::pty::{PtyInstruction, ReplacedPane};
use yinz_bus::screen::ScreenInstruction;
use yinz_types::data::Event;

#[test]
fn selectable_panes_except_target_only_counts_overlays_that_retain_the_tab() {
    let mut tab = create_new_tab(Size { cols: 80, rows: 24 }, true);
    tab.close_pane(PaneId::Terminal(TerminalId::new(1)), false, None);
    let [target, other] = [2, 3].map(|id| PaneId::Terminal(TerminalId::new(id)));
    for id in [target, other] {
        tab.new_pane(
            id,
            None,
            None,
            false,
            false,
            NewPanePlacement::Overlay(yinz_types::data::OverlayPlacement::dismissible(None)),
            Some(ClientId::new(1)),
            None,
        )
        .unwrap();
    }
    assert!(!tab.has_selectable_panes());
    assert!(!tab.has_selectable_panes_except(target));
    tab.overlay_panes.retain_tab_for(target);
    assert!(tab.has_selectable_panes());
    assert!(!tab.has_selectable_panes_except(target));
    assert!(tab.has_selectable_panes_except(other));
    tab.overlay_panes.retain_tab_for(other);
    assert!(tab.has_selectable_panes_except(target));
    assert!(tab.has_selectable_panes_except(other));
    tab.set_pane_selectable(other, false);
    assert!(!tab.has_selectable_panes_except(target));
}

#[test]
fn no_ui_fullscreen_editor_exit_reveals_floating_survivor() {
    let mut tab = create_new_tab(
        Size {
            cols: 121,
            rows: 20,
        },
        true,
    );
    let (sender, _receiver) = unbounded();
    tab.senders
        .replace_to_plugin(SenderWithContext::new(sender));
    let client = ClientId::new(1);
    let source = PaneId::Terminal(TerminalId::new(1));
    let plugin = PaneId::Plugin(crate::PluginId::new(2));
    let editor = PaneId::Terminal(TerminalId::new(3));
    let floating = PaneId::Terminal(TerminalId::new(4));
    tab.new_pane(
        plugin,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client),
        None,
    )
    .unwrap();
    tab.set_pane_selectable(plugin, false);
    tab.new_pane(
        floating,
        None,
        None,
        false,
        true,
        NewPanePlacement::Floating(None),
        Some(client),
        None,
    )
    .unwrap();
    tab.hide_floating_panes();
    tab.close_pane(source, false, Some(0));
    tab.new_in_place_pane(
        editor,
        None,
        None,
        Some(plugin),
        false,
        Some(client),
        None,
        None,
    )
    .unwrap();
    tab.focus_pane_with_id(editor, false, false, client)
        .unwrap();
    tab.tiled_panes.toggle_pane_no_ui_fullscreen(editor);
    tab.ensure_selectable_focus(client).unwrap();
    assert!(
        tab.tiled_panes.fullscreen_is_active(),
        "valid fullscreen focus is preserved"
    );
    assert_eq!(tab.get_active_pane_id(client), Some(editor));
    tab.close_pane(editor, false, Some(0));
    tab.ensure_selectable_focus(client).unwrap();
    assert!(tab.has_selectable_panes());
    assert_eq!(
        tab.get_active_pane_id(client),
        Some(floating),
        "floating visible {}, no-UI fullscreen {}",
        tab.are_floating_panes_visible(),
        tab.tiled_panes.fullscreen_covers_ui()
    );
    assert!(tab.are_floating_panes_visible());
    assert!(!tab.tiled_panes.fullscreen_covers_ui());
    assert!(!tab.tiled_panes.fullscreen_is_active());
    assert!(tab.get_pane_with_id(floating).unwrap().selectable());
    let receiver = install_pty_writer_capture(&mut tab);
    tab.write_to_active_terminal(&None, b"x".to_vec(), false, client, None)
        .unwrap();
    assert!(matches!(drain_pty_writer(&receiver).as_slice(),
        [PtyWriteInstruction::Write(bytes, terminal, _)]
            if bytes == b"x" && *terminal == TerminalId::new(4)));
    tab.assert_invariants();
}

#[test]
fn middle_replacement_exit_preserves_restoration_and_explicit_chain_destruction() {
    for (stacked, hidden_member) in [(false, false), (true, false), (true, true)] {
        for scrollback_editor in [false, true] {
            for explicit in [false, true] {
                let mut tab = create_new_tab(
                    Size {
                        cols: 121,
                        rows: 40,
                    },
                    true,
                );
                let (sender, receiver) = unbounded();
                tab.senders
                    .replace_to_plugin(SenderWithContext::new(sender));
                let client = ClientId::new(1);
                let source = PaneId::Terminal(TerminalId::new(1));
                let middle = PaneId::Plugin(crate::PluginId::new(2));
                let successor = PaneId::Terminal(TerminalId::new(3));
                let neighbor = PaneId::Terminal(TerminalId::new(4));
                if stacked {
                    *tab.stacked_pane_list.borrow_mut() = true;
                    tab.new_pane(
                        neighbor,
                        None,
                        None,
                        false,
                        true,
                        NewPanePlacement::Stacked {
                            pane_id_to_stack_under: Some(source),
                            borderless: None,
                        },
                        Some(client),
                        None,
                    )
                    .unwrap();
                    tab.focus_pane_with_id(source, false, false, client)
                        .unwrap();
                    assert!(tab.pane_is_stack_list_member(&source));
                }
                tab.suppress_pane_and_replace_with_pid(
                    source,
                    middle,
                    ReplacedPane::Suppress,
                    None,
                    None,
                    None,
                )
                .unwrap();
                if scrollback_editor {
                    tab.replace_pane_with_editor_pane(successor, middle)
                        .unwrap();
                } else {
                    tab.suppress_pane_and_replace_with_pid(
                        middle,
                        successor,
                        ReplacedPane::Suppress,
                        None,
                        None,
                        None,
                    )
                    .unwrap();
                }
                let restoration = tab.suppressed_panes[&middle].restored_by;
                if hidden_member {
                    tab.focus_pane_with_id(neighbor, false, false, client)
                        .unwrap();
                }
                tab.close_pane(middle, false, Some(0));
                assert!(!tab.has_pane_with_pid(&middle));
                assert_eq!(tab.suppressed_panes[&source].restored_by, restoration);
                assert_eq!(
                    tab.tiled_panes.get_pane(successor).is_some(),
                    !hidden_member
                );
                assert_eq!(
                    tab.scrollback_editor_subject(successor).is_some(),
                    scrollback_editor
                );
                if explicit {
                    let closed = tab.close_pane_explicitly(successor, None);
                    assert_eq!(closed.len(), 2);
                    assert!(!tab.has_pane_with_pid(&source));
                } else {
                    tab.close_pane(successor, false, Some(0));
                    assert_eq!(tab.tiled_panes.get_pane(source).is_some(), !hidden_member);
                    if stacked {
                        assert!(tab.pane_is_stack_list_member(&source));
                    }
                    if hidden_member {
                        assert!(tab.suppressed_panes[&source].restored_by.is_none());
                        tab.focus_pane_with_id(source, false, false, client)
                            .unwrap();
                        assert!(tab.tiled_panes.get_pane(source).is_some());
                    }
                }
                assert!(!tab.has_pane_with_pid(&successor));
                assert_eq!(
                    tab.get_all_pane_ids().len(),
                    usize::from(stacked) + usize::from(!explicit)
                );
                // A duplicate exit must not announce a pane that no longer exists.
                tab.close_pane(middle, false, None);
                let closed: Vec<_> = receiver
                    .try_iter()
                    .flat_map(|(instruction, _)| match instruction {
                        PluginInstruction::Update(events) => events
                            .into_iter()
                            .filter_map(|(_, _, event)| match event {
                                Event::PaneClosed(id) => Some(id),
                                _ => None,
                            })
                            .collect::<Vec<_>>(),
                        _ => vec![],
                    })
                    .collect();
                for id in [middle, successor] {
                    assert_eq!(closed.iter().filter(|closed| **closed == id).count(), 1);
                }
                assert_eq!(
                    closed.iter().filter(|closed| **closed == source).count(),
                    usize::from(explicit)
                );
                assert_eq!(closed.len(), 2 + usize::from(explicit));
                tab.assert_invariants();
            }
        }
    }
}

#[test]
fn exiting_parked_replacement_parks_every_predecessor_without_dangling_links() {
    let mut tab = create_new_tab(
        Size {
            cols: 121,
            rows: 20,
        },
        true,
    );
    let source = PaneId::Terminal(TerminalId::new(1));
    let other = PaneId::Terminal(TerminalId::new(2));
    let replacement = PaneId::Terminal(TerminalId::new(3));
    tab.vertical_split(other, None, None, ClientId::new(1), None, None)
        .unwrap();
    tab.suppress_pane_and_replace_with_pid(
        source,
        replacement,
        ReplacedPane::Suppress,
        None,
        None,
        None,
    )
    .unwrap();
    tab.suppress_pane(other, None);
    tab.suppressed_panes.get_mut(&other).unwrap().restored_by =
        tab.suppressed_panes[&source].restored_by;
    tab.suppress_pane(replacement, None);
    tab.close_pane(replacement, false, None);
    for id in [source, other] {
        assert!(tab.suppressed_panes[&id].restored_by.is_none());
    }
    assert_eq!(tab.get_all_pane_ids().len(), 2);
    tab.assert_invariants();
}

#[test]
fn exiting_last_tiled_pane_reveals_and_focuses_hidden_floating_panes() {
    let mut tab = create_tab_with_two_floating_panes();
    let clients = [ClientId::new(1), ClientId::new(2)];
    tab.add_client(clients[1], None).unwrap();
    tab.hide_floating_panes();
    tab.close_pane(PaneId::Terminal(TerminalId::new(1)), false, None);
    assert!(tab.has_selectable_panes());
    assert!(tab.are_floating_panes_visible());
    for client in clients {
        let focused = tab
            .get_active_pane_id(client)
            .expect("retained tab has usable focus");
        assert!(tab.floating_panes.get_pane(focused).unwrap().selectable());
    }
}

#[test]
fn focus_repair_preserves_a_modal_until_it_closes() {
    let mut tab = create_tab_with_two_floating_panes();
    let client = ClientId::new(1);
    let modal = PaneId::Terminal(TerminalId::new(4));
    tab.hide_floating_panes();
    tab.new_pane(
        modal,
        None,
        None,
        false,
        true,
        NewPanePlacement::Overlay(yinz_types::data::OverlayPlacement {
            background_opacity: None,
            kind: yinz_types::data::OverlayKind::Modal,
            ..Default::default()
        }),
        Some(client),
        None,
    )
    .unwrap();
    tab.close_pane(PaneId::Terminal(TerminalId::new(1)), false, None);
    tab.ensure_selectable_focus(client).unwrap();
    assert_eq!(tab.get_active_pane_id(client), Some(modal));
    assert!(!tab.are_floating_panes_visible());
    tab.close_pane(modal, false, None);
    assert!(tab.are_floating_panes_visible());
    let focused = tab.get_active_pane_id(client).unwrap();
    assert!(tab.floating_panes.get_pane(focused).unwrap().selectable());
}

#[test]
fn fullscreen_hidden_replacement_survives_natural_exit_and_explicit_close() {
    for close_owner in [false, true] {
        for explicit in [false, true] {
            let mut tab = create_new_tab(
                Size {
                    cols: 121,
                    rows: 20,
                },
                true,
            );
            let (sender, _receiver) = unbounded();
            tab.senders
                .replace_to_plugin(SenderWithContext::new(sender));
            let owner = PaneId::Terminal(TerminalId::new(1));
            let hidden = PaneId::Terminal(TerminalId::new(2));
            let replacement = PaneId::Plugin(crate::PluginId::new(3));
            tab.vertical_split(hidden, None, None, ClientId::new(1), None, None)
                .unwrap();
            tab.tiled_panes.toggle_pane_fullscreen(owner);
            tab.suppress_pane_and_replace_with_pid(
                hidden,
                replacement,
                ReplacedPane::Suppress,
                None,
                None,
                None,
            )
            .unwrap();
            assert!(!tab.tiled_panes.panes_to_hide_contains(hidden));
            assert!(tab.tiled_panes.panes_to_hide_contains(replacement));
            assert_eq!(tab.tiled_panes.fullscreen_pane_id(), Some(owner));
            let closing = if close_owner { owner } else { replacement };
            if explicit {
                tab.close_pane_explicitly(closing, None);
            } else {
                tab.close_pane(closing, false, None);
            }
            assert!(tab.has_selectable_panes());
            if close_owner {
                assert!(tab.tiled_panes.get_pane(replacement).is_some());
                tab.close_pane(replacement, false, None);
                assert!(tab.tiled_panes.get_pane(hidden).is_some());
            } else if !explicit {
                assert!(tab.tiled_panes.panes_to_hide_contains(hidden));
                assert!(!tab.tiled_panes.panes_to_hide_contains(replacement));
            } else {
                assert_eq!(tab.get_all_pane_ids(), vec![owner]);
            }
            tab.tiled_panes.unset_fullscreen();
            assert_eq!(tab.panes_to_hide_count(), 0);
            assert!(!tab.tiled_panes.fullscreen_is_active());
            tab.assert_invariants();
        }
    }
}

#[test]
fn explicit_close_destroys_the_entire_live_replacement_chain() {
    for targeted in [None, Some(0), Some(1), Some(2)] {
        let mut tab = create_new_tab(
            Size {
                cols: 121,
                rows: 20,
            },
            true,
        );
        let (sender, receiver) = unbounded();
        tab.senders
            .replace_to_plugin(SenderWithContext::new(sender));
        let source = PaneId::Terminal(TerminalId::new(1));
        let view = PaneId::Plugin(crate::PluginId::new(2));
        let helper = PaneId::Terminal(TerminalId::new(3));
        tab.suppress_pane_and_replace_with_pid(
            source,
            view,
            ReplacedPane::Suppress,
            None,
            None,
            None,
        )
        .unwrap();
        tab.suppress_pane_and_replace_with_pid(
            view,
            helper,
            ReplacedPane::Suppress,
            None,
            None,
            None,
        )
        .unwrap();
        if let Some(index) = targeted {
            let ids = [source, view, helper];
            assert_eq!(tab.close_pane_explicitly(ids[index], None).len(), 3);
            assert!(tab.close_pane_explicitly(ids[index], None).is_empty());
        } else {
            tab.close_focused_pane(ClientId::new(1), None).unwrap();
        }
        assert!(
            tab.get_all_pane_ids().is_empty(),
            "explicit close left hidden panes alive"
        );
        let closed: Vec<_> = receiver
            .try_iter()
            .flat_map(|(instruction, _)| match instruction {
                PluginInstruction::Update(events) => events
                    .into_iter()
                    .filter_map(|(_, _, event)| match event {
                        Event::PaneClosed(id) => Some(id),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
                _ => vec![],
            })
            .collect();
        for id in [source, view, helper] {
            assert_eq!(closed.iter().filter(|closed| **closed == id).count(), 1);
        }
    }
}

#[test]
fn explicit_close_of_any_multipane_replacement_destroys_both_chains() {
    use yinz_types::multipane::*;
    let chains = [
        [
            PaneId::Terminal(TerminalId::new(2)),
            PaneId::Plugin(crate::PluginId::new(4)),
            PaneId::Terminal(TerminalId::new(6)),
        ],
        [
            PaneId::Terminal(TerminalId::new(3)),
            PaneId::Plugin(crate::PluginId::new(5)),
            PaneId::Terminal(TerminalId::new(7)),
        ],
    ];
    let mut expected: Vec<_> = chains.into_iter().flatten().collect();
    expected.sort();
    for targeted in &expected {
        let mut tab = create_new_tab(
            Size {
                cols: 120,
                rows: 40,
            },
            true,
        );
        let client = ClientId::new(1);
        let (plugin_sender, plugin_receiver) = unbounded();
        tab.senders
            .replace_to_plugin(SenderWithContext::new(plugin_sender));
        let (pty_sender, pty_receiver) = unbounded();
        tab.senders
            .replace_to_pty(SenderWithContext::new(pty_sender));
        let (screen_sender, screen_receiver) = unbounded();
        tab.senders.to_screen = Some(SenderWithContext::new(screen_sender));
        let placement = OverlayPlacement::dismissible(None);
        for chain in chains {
            tab.new_pane(
                chain[0],
                None,
                None,
                false,
                true,
                NewPanePlacement::Overlay(placement.clone()),
                Some(client),
                None,
            )
            .unwrap();
        }
        tab.create_multipane(
            [chains[0][0], chains[1][0]],
            MultipaneSplit {
                direction: MultipaneDirection::LeftRight,
                ratio: 5000,
            },
            0,
            placement,
        )
        .unwrap();
        for chain in chains {
            for pair in chain.windows(2) {
                tab.suppress_pane_and_replace_with_pid(
                    pair[0],
                    pair[1],
                    ReplacedPane::Suppress,
                    None,
                    None,
                    None,
                )
                .unwrap();
            }
        }
        tab.focus_pane_with_id(chains[0][2], false, false, client)
            .unwrap();

        let mut closed = tab.close_pane_explicitly(*targeted, None);
        closed.sort();
        assert_eq!(closed, expected, "closing {targeted:?}");
        assert_eq!(tab.overlay_panes.len(), 0);
        assert!(tab.suppressed_panes.is_empty());
        let survivor = PaneId::Terminal(TerminalId::new(1));
        assert_eq!(tab.get_all_pane_ids(), vec![survivor]);
        assert_eq!(tab.get_active_pane_id(client), Some(survivor));
        for id in &expected {
            assert!(tab.close_pane_explicitly(*id, None).is_empty());
        }
        let mut announced: Vec<_> = plugin_receiver
            .try_iter()
            .flat_map(|(instruction, _)| match instruction {
                PluginInstruction::Update(events) => events
                    .into_iter()
                    .filter_map(|(_, _, event)| match event {
                        Event::PaneClosed(id) => Some(id),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
                _ => vec![],
            })
            .collect();
        announced.sort();
        assert_eq!(announced, expected);
        let mut notified: Vec<_> = screen_receiver
            .try_iter()
            .filter_map(|(instruction, _)| match instruction {
                ScreenInstruction::NotifyPaneClosedToSubscribers { pane_id } => Some(pane_id),
                _ => None,
            })
            .collect();
        notified.sort();
        assert_eq!(notified, expected);
        let mut terminated: Vec<_> = pty_receiver
            .try_iter()
            .filter_map(|(instruction, _)| match instruction {
                PtyInstruction::ClosePane(id, _) => Some(id),
                _ => None,
            })
            .collect();
        terminated.sort();
        assert_eq!(terminated, expected);
        tab.assert_invariants();
    }
}

/// Giving up one multipane child leaves the sibling as the container's only pane and tells the
/// subscribers it closed, but asks the pty for nothing: the caller decides what the removed
/// pane's process or plugin is owed. A member that stands in for a suppressed pane is left to
/// `close_pane`, which restores what it hides.
#[test]
fn removing_a_multipane_member_keeps_its_sibling_and_sends_nothing_to_the_pty() {
    use yinz_types::multipane::*;
    let mut tab = create_new_tab(
        Size {
            cols: 120,
            rows: 40,
        },
        true,
    );
    let client = ClientId::new(1);
    let (plugin_sender, plugin_receiver) = unbounded();
    tab.senders
        .replace_to_plugin(SenderWithContext::new(plugin_sender));
    let (pty_sender, pty_receiver) = unbounded();
    tab.senders
        .replace_to_pty(SenderWithContext::new(pty_sender));
    let (screen_sender, screen_receiver) = unbounded();
    tab.senders.to_screen = Some(SenderWithContext::new(screen_sender));
    let owner = PaneId::Plugin(crate::PluginId::new(4));
    let slot = PaneId::Terminal(TerminalId::new(5));
    let placement = OverlayPlacement::dismissible(None);
    for pane in [owner, slot] {
        tab.new_pane(
            pane,
            None,
            None,
            false,
            true,
            NewPanePlacement::Overlay(placement.clone()),
            Some(client),
            None,
        )
        .unwrap();
    }
    tab.create_multipane(
        [owner, slot],
        MultipaneSplit {
            direction: MultipaneDirection::LeftRight,
            ratio: 5000,
        },
        0,
        placement,
    )
    .unwrap();
    tab.focus_pane_with_id(slot, false, false, client).unwrap();

    assert!(tab.remove_multipane_member(slot));

    assert!(!tab.has_pane_with_pid(&slot));
    assert!(tab.pane_is_overlay(owner));
    assert_eq!(tab.multipane_info(owner), None);
    assert_eq!(tab.get_active_pane_id(client), Some(owner));
    assert!(
        !tab.remove_multipane_member(owner),
        "the owner alone is no multipane member"
    );
    assert!(tab.has_pane_with_pid(&owner));
    assert!(pty_receiver
        .try_iter()
        .all(|(instruction, _)| !matches!(instruction, PtyInstruction::ClosePane(..))));
    assert!(plugin_receiver.try_iter().any(|(instruction, _)| matches!(
        instruction,
        PluginInstruction::Update(events)
            if events.iter().any(|(_, _, event)| matches!(event, Event::PaneClosed(id) if *id == slot))
    )));
    assert!(screen_receiver.try_iter().any(|(instruction, _)| matches!(
        instruction,
        ScreenInstruction::NotifyPaneClosedToSubscribers { pane_id } if pane_id == slot
    )));
    tab.assert_invariants();

    // A member that holds the place of a suppressed pane is not given up here.
    let second = PaneId::Terminal(TerminalId::new(6));
    let stand_in = PaneId::Terminal(TerminalId::new(7));
    let placement = OverlayPlacement::dismissible(None);
    tab.new_pane(
        second,
        None,
        None,
        false,
        true,
        NewPanePlacement::Overlay(placement.clone()),
        Some(client),
        None,
    )
    .unwrap();
    tab.create_multipane(
        [owner, second],
        MultipaneSplit {
            direction: MultipaneDirection::LeftRight,
            ratio: 5000,
        },
        0,
        placement,
    )
    .unwrap();
    tab.suppress_pane_and_replace_with_pid(
        second,
        stand_in,
        ReplacedPane::Suppress,
        None,
        None,
        None,
    )
    .unwrap();
    assert!(!tab.remove_multipane_member(stand_in));
    assert!(tab.multipane_info(stand_in).is_some());
}

#[test]
fn pane_replacement_keeps_the_original_suppressed_until_explicit_restart() {
    for restart in [false, true] {
        let mut tab = create_new_tab(
            Size {
                cols: 121,
                rows: 20,
            },
            true,
        );
        let (plugin_sender, _plugin_receiver) = unbounded();
        tab.senders
            .replace_to_plugin(SenderWithContext::new(plugin_sender));
        let source = PaneId::Terminal(TerminalId::new(1));
        let view = PaneId::Plugin(crate::PluginId::new(2));
        let worker = PaneId::Terminal(TerminalId::new(3));
        tab.hold_pane(source, Some(0), false, RunCommand::default());
        tab.suppress_pane_and_replace_with_pid(
            source,
            view,
            ReplacedPane::Suppress,
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            tab.suppressed_panes[&source].restored_by.unwrap().pane_id,
            view
        );

        // Park the held source before presenting another pane.
        tab.suppress_pane(source, None);
        tab.vertical_split(worker, None, None, ClientId::new(1), None, None)
            .unwrap();
        tab.suppress_pane(worker, None);
        let worker_pane = tab.extract_pane(worker, true).unwrap();
        tab.suppress_pane_and_replace_with_other_pane(view, worker_pane, None);
        assert!(tab.suppressed_panes[&source].restored_by.is_none());
        let view_pane = tab.extract_pane(view, true).unwrap();
        tab.suppress_pane_and_replace_with_other_pane(worker, view_pane, None);
        assert!(tab.tiled_panes.get_pane(view).is_some());
        assert!(tab.suppressed_panes[&source].restored_by.is_none());

        if restart {
            let source_pane = tab.extract_pane(source, true).unwrap();
            tab.suppress_pane_and_replace_with_other_pane(view, source_pane, None);
        }
        tab.close_pane(view, false, None);
        assert_eq!(tab.tiled_panes.get_pane(source).is_some(), restart);
        assert_eq!(tab.suppressed_panes.contains_key(&source), !restart);
        if !restart {
            assert!(tab.suppressed_panes[&source].restored_by.is_none());
            tab.close_pane(worker, false, None);
            assert!(tab.suppressed_panes.contains_key(&source));
        }
    }
}

#[test]
pub fn close_pane_with_another_pane_above_it() {
    // ┌───────────┐            ┌───────────┐
    // │xxxxxxxxxxx│            │xxxxxxxxxxx│
    // │xxxxxxxxxxx│            │xxxxxxxxxxx│
    // ├───────────┤ ==close==> │xxxxxxxxxxx│
    // │███████████│            │xxxxxxxxxxx│
    // │███████████│            │xxxxxxxxxxx│
    // └───────────┘            └───────────┘
    // █ == pane being closed

    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    tab.horizontal_split(new_pane_id, None, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.close_focused_pane(crate::ClientId::new(1), None)
        .unwrap();
    assert_eq!(tab.tiled_panes.panes.len(), 1, "One pane left in tab");

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "remaining pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "remaining pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        121,
        "remaining pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        20,
        "remaining pane row count"
    );
}

#[test]
pub fn close_pane_with_another_pane_below_it() {
    // ┌───────────┐            ┌───────────┐
    // │███████████│            │xxxxxxxxxxx│
    // │███████████│            │xxxxxxxxxxx│
    // ├───────────┤ ==close==> │xxxxxxxxxxx│
    // │xxxxxxxxxxx│            │xxxxxxxxxxx│
    // │xxxxxxxxxxx│            │xxxxxxxxxxx│
    // └───────────┘            └───────────┘
    // █ == pane being closed

    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    tab.horizontal_split(new_pane_id, None, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();
    tab.close_focused_pane(crate::ClientId::new(1), None)
        .unwrap();
    assert_eq!(tab.tiled_panes.panes.len(), 1, "One pane left in tab");

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "remaining pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "remaining pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        121,
        "remaining pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        20,
        "remaining pane row count"
    );
}

#[test]
pub fn close_pane_with_another_pane_to_the_left() {
    // ┌─────┬─────┐            ┌──────────┐
    // │xxxxx│█████│            │xxxxxxxxxx│
    // │xxxxx│█████│ ==close==> │xxxxxxxxxx│
    // │xxxxx│█████│            │xxxxxxxxxx│
    // └─────┴─────┘            └──────────┘
    // █ == pane being closed
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    tab.vertical_split(new_pane_id, None, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.close_focused_pane(crate::ClientId::new(1), None)
        .unwrap();
    assert_eq!(tab.tiled_panes.panes.len(), 1, "One pane left in tab");

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "remaining pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "remaining pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        121,
        "remaining pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        20,
        "remaining pane row count"
    );
}

#[test]
pub fn close_pane_with_another_pane_to_the_right() {
    // ┌─────┬─────┐            ┌──────────┐
    // │█████│xxxxx│            │xxxxxxxxxx│
    // │█████│xxxxx│ ==close==> │xxxxxxxxxx│
    // │█████│xxxxx│            │xxxxxxxxxx│
    // └─────┴─────┘            └──────────┘
    // █ == pane being closed
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    tab.vertical_split(new_pane_id, None, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_focus_left(crate::ClientId::new(1)).unwrap();
    tab.close_focused_pane(crate::ClientId::new(1), None)
        .unwrap();
    assert_eq!(tab.tiled_panes.panes.len(), 1, "One pane left in tab");

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "remaining pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "remaining pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        121,
        "remaining pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        20,
        "remaining pane row count"
    );
}

#[test]
pub fn close_pane_with_multiple_panes_above_it() {
    // ┌─────┬─────┐            ┌─────┬─────┐
    // │xxxxx│xxxxx│            │xxxxx│xxxxx│
    // │xxxxx│xxxxx│            │xxxxx│xxxxx│
    // ├─────┴─────┤ ==close==> │xxxxx│xxxxx│
    // │███████████│            │xxxxx│xxxxx│
    // │███████████│            │xxxxx│xxxxx│
    // └───────────┘            └─────┴─────┘
    // █ == pane being closed
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    tab.horizontal_split(
        new_pane_id_1,
        None,
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();
    tab.vertical_split(
        new_pane_id_2,
        None,
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_down(crate::ClientId::new(1)).unwrap();
    tab.close_focused_pane(crate::ClientId::new(1), None)
        .unwrap();
    assert_eq!(tab.tiled_panes.panes.len(), 2, "Two panes left in tab");

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "first remaining pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "first remaining pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        61,
        "first remaining pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        20,
        "first remaining pane row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .x,
        61,
        "second remaining pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "second remaining pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        60,
        "second remaining pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        20,
        "second remaining pane row count"
    );
}

#[test]
pub fn close_pane_with_multiple_panes_below_it() {
    // ┌───────────┐            ┌─────┬─────┐
    // │███████████│            │xxxxx│xxxxx│
    // │███████████│            │xxxxx│xxxxx│
    // ├─────┬─────┤ ==close==> │xxxxx│xxxxx│
    // │xxxxx│xxxxx│            │xxxxx│xxxxx│
    // │xxxxx│xxxxx│            │xxxxx│xxxxx│
    // └─────┴─────┘            └─────┴─────┘
    // █ == pane being closed
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    tab.horizontal_split(
        new_pane_id_1,
        None,
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.vertical_split(
        new_pane_id_2,
        None,
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();
    tab.close_focused_pane(crate::ClientId::new(1), None)
        .unwrap();
    assert_eq!(tab.tiled_panes.panes.len(), 2, "Two panes left in tab");

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "first remaining pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "first remaining pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        61,
        "first remaining pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        20,
        "first remaining pane row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .x,
        61,
        "second remaining pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "second remaining pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        60,
        "second remaining pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        20,
        "second remaining pane row count"
    );
}

#[test]
pub fn close_pane_with_multiple_panes_to_the_left() {
    // ┌─────┬─────┐            ┌──────────┐
    // │xxxxx│█████│            │xxxxxxxxxx│
    // │xxxxx│█████│            │xxxxxxxxxx│
    // ├─────┤█████│ ==close==> ├──────────┤
    // │xxxxx│█████│            │xxxxxxxxxx│
    // │xxxxx│█████│            │xxxxxxxxxx│
    // └─────┴─────┘            └──────────┘
    // █ == pane being closed
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    tab.vertical_split(
        new_pane_id_1,
        None,
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_left(crate::ClientId::new(1)).unwrap();
    tab.horizontal_split(
        new_pane_id_2,
        None,
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_right(crate::ClientId::new(1)).unwrap();
    tab.close_focused_pane(crate::ClientId::new(1), None)
        .unwrap();
    assert_eq!(tab.tiled_panes.panes.len(), 2, "Two panes left in tab");

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "first remaining pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "first remaining pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        121,
        "first remaining pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        10,
        "first remaining pane row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "second remaining pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .y,
        10,
        "second remaining pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        121,
        "second remaining pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        10,
        "second remaining pane row count"
    );
}

#[test]
pub fn close_pane_with_multiple_panes_to_the_right() {
    // ┌─────┬─────┐            ┌──────────┐
    // │█████│xxxxx│            │xxxxxxxxxx│
    // │█████│xxxxx│            │xxxxxxxxxx│
    // │█████├─────┤ ==close==> ├──────────┤
    // │█████│xxxxx│            │xxxxxxxxxx│
    // │█████│xxxxx│            │xxxxxxxxxx│
    // └─────┴─────┘            └──────────┘
    // █ == pane being closed
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    tab.vertical_split(
        new_pane_id_1,
        None,
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.horizontal_split(
        new_pane_id_2,
        None,
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_left(crate::ClientId::new(1)).unwrap();
    tab.close_focused_pane(crate::ClientId::new(1), None)
        .unwrap();
    assert_eq!(tab.tiled_panes.panes.len(), 2, "Two panes left in tab");

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "first remaining pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "first remaining pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        121,
        "first remaining pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        10,
        "first remaining pane row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "second remaining pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .y,
        10,
        "second remaining pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        121,
        "second remaining pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        10,
        "second remaining pane row count"
    );
}

#[test]
pub fn close_pane_with_multiple_panes_above_it_away_from_screen_edges() {
    // ┌───┬───┬───┬───┐            ┌───┬───┬───┬───┐
    // │xxx│xxx│xxx│xxx│            │xxx│xxx│xxx│xxx│
    // ├───┤xxx│xxx├───┤            ├───┤xxx│xxx├───┤
    // │xxx├───┴───┤xxx│ ==close==> │xxx│xxx│xxx│xxx│
    // │xxx│███████│xxx│            │xxx│xxx│xxx│xxx│
    // │xxx│███████│xxx│            │xxx│xxx│xxx│xxx│
    // └───┴───────┴───┘            └───┴───┴───┴───┘
    // █ == pane being closed
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));
    let new_pane_id_4 = PaneId::Terminal(crate::TerminalId::new(5));
    let new_pane_id_5 = PaneId::Terminal(crate::TerminalId::new(6));
    let new_pane_id_6 = PaneId::Terminal(crate::TerminalId::new(7));

    tab.vertical_split(
        new_pane_id_1,
        None,
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.vertical_split(
        new_pane_id_2,
        None,
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_left(crate::ClientId::new(1)).unwrap();
    tab.move_focus_left(crate::ClientId::new(1)).unwrap();
    tab.horizontal_split(
        new_pane_id_3,
        None,
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_right(crate::ClientId::new(1)).unwrap();
    tab.horizontal_split(
        new_pane_id_4,
        None,
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_right(crate::ClientId::new(1)).unwrap();
    tab.horizontal_split(
        new_pane_id_5,
        None,
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_left(crate::ClientId::new(1)).unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();
    tab_resize_down(&mut tab, crate::ClientId::new(1));
    tab.vertical_split(
        new_pane_id_6,
        None,
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_down(crate::ClientId::new(1)).unwrap();
    tab.close_focused_pane(crate::ClientId::new(1), None)
        .unwrap();

    assert_eq!(tab.tiled_panes.panes.len(), 6, "Six panes left in tab");

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "first remaining pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "first remaining pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        61,
        "first remaining pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        10,
        "first remaining pane row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .x,
        61,
        "second remaining pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "second remaining pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        15,
        "second remaining pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        20,
        "second remaining pane row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .x,
        91,
        "third remaining pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "third remaining pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        30,
        "third remaining pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        10,
        "third remaining pane row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "fourth remaining pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .y,
        10,
        "fourth remaining pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        61,
        "fourth remaining pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        10,
        "fourth remaining pane row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(6)))
            .unwrap()
            .position_and_size()
            .x,
        91,
        "sixths remaining pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(6)))
            .unwrap()
            .position_and_size()
            .y,
        10,
        "sixths remaining pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(6)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        30,
        "sixths remaining pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(6)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        10,
        "sixths remaining pane row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(7)))
            .unwrap()
            .position_and_size()
            .x,
        76,
        "seventh remaining pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(7)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "seventh remaining pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(7)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        15,
        "seventh remaining pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(7)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        20,
        "seventh remaining pane row count"
    );
}

#[test]
pub fn close_pane_with_multiple_panes_below_it_away_from_screen_edges() {
    // ┌───┬───────┬───┐            ┌───┬───┬───┬───┐
    // │xxx│███████│xxx│            │xxx│xxx│xxx│xxx│
    // │xxx│███████│xxx│            │xxx│xxx│xxx│xxx│
    // │xxx├───┬───┤xxx│ ==close==> │xxx│xxx│xxx│xxx│
    // ├───┤xxx│xxx├───┤            ├───┤xxx│xxx├───┤
    // │xxx│xxx│xxx│xxx│            │xxx│xxx│xxx│xxx│
    // └───┴───┴───┴───┘            └───┴───┴───┴───┘
    // █ == pane being closed

    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));
    let new_pane_id_4 = PaneId::Terminal(crate::TerminalId::new(5));
    let new_pane_id_5 = PaneId::Terminal(crate::TerminalId::new(6));
    let new_pane_id_6 = PaneId::Terminal(crate::TerminalId::new(7));

    tab.vertical_split(
        new_pane_id_1,
        None,
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.vertical_split(
        new_pane_id_2,
        None,
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_left(crate::ClientId::new(1)).unwrap();
    tab.move_focus_left(crate::ClientId::new(1)).unwrap();
    tab.horizontal_split(
        new_pane_id_3,
        None,
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_right(crate::ClientId::new(1)).unwrap();
    tab.horizontal_split(
        new_pane_id_4,
        None,
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_right(crate::ClientId::new(1)).unwrap();
    tab.horizontal_split(
        new_pane_id_5,
        None,
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_left(crate::ClientId::new(1)).unwrap();
    tab_resize_up(&mut tab, crate::ClientId::new(1));
    tab.vertical_split(
        new_pane_id_6,
        None,
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();
    tab.close_focused_pane(crate::ClientId::new(1), None)
        .unwrap();

    assert_eq!(tab.tiled_panes.panes.len(), 6, "Six panes left in tab");

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "first remaining pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "first remaining pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        61,
        "first remaining pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        10,
        "first remaining pane row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .x,
        91,
        "third remaining pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "third remaining pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        30,
        "third remaining pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        10,
        "third remaining pane row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "fourth remaining pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .y,
        10,
        "fourth remaining pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        61,
        "fourth remaining pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        10,
        "fourth remaining pane row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(5)))
            .unwrap()
            .position_and_size()
            .x,
        61,
        "second remaining pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(5)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "second remaining pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(5)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        15,
        "second remaining pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(5)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        20,
        "second remaining pane row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(6)))
            .unwrap()
            .position_and_size()
            .x,
        91,
        "sixths remaining pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(6)))
            .unwrap()
            .position_and_size()
            .y,
        10,
        "sixths remaining pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(6)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        30,
        "sixths remaining pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(6)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        10,
        "sixths remaining pane row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(7)))
            .unwrap()
            .position_and_size()
            .x,
        76,
        "seventh remaining pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(7)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "seventh remaining pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(7)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        15,
        "seventh remaining pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(7)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        20,
        "seventh remaining pane row count"
    );
}

#[test]
pub fn close_pane_with_multiple_panes_to_the_left_away_from_screen_edges() {
    // ┌────┬──────┐            ┌────┬──────┐
    // │xxxx│xxxxxx│            │xxxx│xxxxxx│
    // ├────┴┬─────┤            ├────┴──────┤
    // │xxxxx│█████│            │xxxxxxxxxxx│
    // ├─────┤█████│ ==close==> ├───────────┤
    // │xxxxx│█████│            │xxxxxxxxxxx│
    // ├────┬┴─────┤            ├────┬──────┤
    // │xxxx│xxxxxx│            │xxxx│xxxxxx│
    // └────┴──────┘            └────┴──────┘
    // █ == pane being closed

    let size = Size {
        cols: 121,
        rows: 30,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));
    let new_pane_id_4 = PaneId::Terminal(crate::TerminalId::new(5));
    let new_pane_id_5 = PaneId::Terminal(crate::TerminalId::new(6));
    let new_pane_id_6 = PaneId::Terminal(crate::TerminalId::new(7));

    tab.horizontal_split(
        new_pane_id_1,
        None,
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.horizontal_split(
        new_pane_id_2,
        None,
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();
    tab.vertical_split(
        new_pane_id_3,
        None,
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_down(crate::ClientId::new(1)).unwrap();
    tab.vertical_split(
        new_pane_id_4,
        None,
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_down(crate::ClientId::new(1)).unwrap();
    tab.vertical_split(
        new_pane_id_5,
        None,
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();
    tab.move_focus_left(crate::ClientId::new(1)).unwrap();
    tab_resize_right(&mut tab, crate::ClientId::new(1));
    tab_resize_up(&mut tab, crate::ClientId::new(1));
    tab_resize_up(&mut tab, crate::ClientId::new(1));
    tab.horizontal_split(
        new_pane_id_6,
        None,
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_right(crate::ClientId::new(1)).unwrap();
    tab.close_focused_pane(crate::ClientId::new(1), None)
        .unwrap();

    assert_eq!(tab.tiled_panes.panes.len(), 6, "Six panes left in tab");

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "first remaining pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "first remaining pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        61,
        "first remaining pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        12,
        "first remaining pane row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "third remaining pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .y,
        12,
        "third remaining pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        121,
        "third remaining pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        5,
        "third remaining pane row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "fourth remaining pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .y,
        22,
        "fourth remaining pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        61,
        "fourth remaining pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        8,
        "fourth remaining pane row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .x,
        61,
        "second remaining pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "second remaining pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        60,
        "second remaining pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        12,
        "second remaining pane row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(6)))
            .unwrap()
            .position_and_size()
            .x,
        61,
        "sixths remaining pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(6)))
            .unwrap()
            .position_and_size()
            .y,
        22,
        "sixths remaining pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(6)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        60,
        "sixths remaining pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(6)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        8,
        "sixths remaining pane row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(7)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "seventh remaining pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(7)))
            .unwrap()
            .position_and_size()
            .y,
        17,
        "seventh remaining pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(7)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        121,
        "seventh remaining pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(7)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        5,
        "seventh remaining pane row count"
    );
}

#[test]
pub fn close_pane_with_multiple_panes_to_the_right_away_from_screen_edges() {
    // ┌────┬──────┐            ┌────┬──────┐
    // │xxxx│xxxxxx│            │xxxx│xxxxxx│
    // ├────┴┬─────┤            ├────┴──────┤
    // │█████│xxxxx│            │xxxxxxxxxxx│
    // │█████├─────┤ ==close==> ├───────────┤
    // │█████│xxxxx│            │xxxxxxxxxxx│
    // ├────┬┴─────┤            ├────┬──────┤
    // │xxxx│xxxxxx│            │xxxx│xxxxxx│
    // └────┴──────┘            └────┴──────┘
    // █ == pane being closed

    let size = Size {
        cols: 121,
        rows: 30,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));
    let new_pane_id_4 = PaneId::Terminal(crate::TerminalId::new(5));
    let new_pane_id_5 = PaneId::Terminal(crate::TerminalId::new(6));
    let new_pane_id_6 = PaneId::Terminal(crate::TerminalId::new(7));

    tab.horizontal_split(
        new_pane_id_1,
        None,
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.horizontal_split(
        new_pane_id_2,
        None,
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();
    tab.vertical_split(
        new_pane_id_3,
        None,
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_down(crate::ClientId::new(1)).unwrap();
    tab.vertical_split(
        new_pane_id_4,
        None,
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_down(crate::ClientId::new(1)).unwrap();
    tab.vertical_split(
        new_pane_id_5,
        None,
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();
    tab_resize_left(&mut tab, crate::ClientId::new(1));
    tab_resize_up(&mut tab, crate::ClientId::new(1));
    tab_resize_up(&mut tab, crate::ClientId::new(1));
    tab.horizontal_split(
        new_pane_id_6,
        None,
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_left(crate::ClientId::new(1)).unwrap();
    tab.close_focused_pane(crate::ClientId::new(1), None)
        .unwrap();

    assert_eq!(tab.tiled_panes.panes.len(), 6, "Six panes left in tab");

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "first remaining pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "first remaining pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        61,
        "first remaining pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        11,
        "first remaining pane row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "fourth remaining pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .y,
        22,
        "fourth remaining pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        61,
        "fourth remaining pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        8,
        "fourth remaining pane row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .x,
        61,
        "second remaining pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "second remaining pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        60,
        "second remaining pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        11,
        "second remaining pane row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(5)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "third remaining pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(5)))
            .unwrap()
            .position_and_size()
            .y,
        11,
        "third remaining pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(5)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        121,
        "third remaining pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(5)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        6,
        "third remaining pane row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(6)))
            .unwrap()
            .position_and_size()
            .x,
        61,
        "sixths remaining pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(6)))
            .unwrap()
            .position_and_size()
            .y,
        22,
        "sixths remaining pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(6)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        60,
        "sixths remaining pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(6)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        8,
        "sixths remaining pane row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(7)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "seventh remaining pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(7)))
            .unwrap()
            .position_and_size()
            .y,
        17,
        "seventh remaining pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(7)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        121,
        "seventh remaining pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(7)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        5,
        "seventh remaining pane row count"
    );
}

// ---------------------------------------------------------------------------
// A client joining a tab that has no tiled panes
//
// `Screen::active_tab_ids` says which tab a client sees and types into; `Tab::connected_clients`
// is what the tab renders for and routes input to. A tab that accepts the first half and
// refuses the second leaves the client with no frames and no working keys, and nothing short of
// a reattach repairs it - so `add_client` must never come back `Ok` having registered nothing.
// A tab with no *tiled* panes is an ordinary state here: floating panes, overlays, a pane
// suppressed behind a hook plugin, a tab still applying its layout.
// ---------------------------------------------------------------------------

#[test]
pub fn a_client_joining_a_tab_with_only_floating_panes_is_connected() {
    let mut tab = create_tab_with_two_floating_panes();
    let joining = crate::ClientId::new(7);
    // Floating panes only: the helper's tab is built around a tiled pane, and it is the tiled
    // count that `add_client` reads.
    tab.close_pane(PaneId::Terminal(crate::TerminalId::new(1)), false, None);
    // The helper's tab already has the default client in it; this is about the case where the
    // joining client is the first one, which is the branch that used to refuse it.
    tab.remove_client(crate::ClientId::new(1));
    assert!(tab.has_no_connected_clients(), "precondition");
    assert!(!tab.tiled_panes.has_panes(), "precondition: no tiled panes");

    tab.add_client(joining, None).expect("the tab takes it");

    assert!(
        tab.has_connected_client(&joining),
        "a tab with no tiled panes still renders for the client and routes its input",
    );
}

#[test]
pub fn a_client_joining_a_tab_with_no_panes_at_all_is_connected() {
    let mut tab = create_new_tab(
        Size {
            cols: 121,
            rows: 20,
        },
        false,
    );
    tab.close_pane(PaneId::Terminal(crate::TerminalId::new(1)), false, None);
    tab.remove_client(crate::ClientId::new(1));
    let joining = crate::ClientId::new(7);

    tab.add_client(joining, None).expect("the tab takes it");

    assert!(
        tab.has_connected_client(&joining),
        "an emptied tab still counts the client, so the sweep can re-home it",
    );
}

#[test]
fn exiting_fullscreen_editor_restoring_nonselectable_plugin_keeps_usable_focus() {
    let mut tab = create_new_tab(
        Size {
            cols: 121,
            rows: 20,
        },
        true,
    );
    let (sender, _receiver) = unbounded();
    tab.senders
        .replace_to_plugin(SenderWithContext::new(sender));
    let client = ClientId::new(1);
    let survivor = PaneId::Terminal(TerminalId::new(1));
    let plugin = PaneId::Plugin(crate::PluginId::new(2));
    let editor = PaneId::Terminal(TerminalId::new(3));
    tab.new_pane(
        plugin,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client),
        None,
    )
    .unwrap();
    tab.set_pane_selectable(plugin, false);

    // A nonselectable plugin can open an editor in its own place and return when it exits.
    tab.new_in_place_pane(
        editor,
        None,
        None,
        Some(plugin),
        false,
        Some(client),
        None,
        None,
    )
    .unwrap();
    tab.focus_pane_with_id(editor, false, false, client)
        .unwrap();
    tab.tiled_panes.toggle_pane_fullscreen(editor);
    tab.ensure_selectable_focus(client).unwrap();
    assert!(
        tab.tiled_panes.fullscreen_is_active(),
        "valid fullscreen focus is preserved"
    );
    assert_eq!(tab.get_active_pane_id(client), Some(editor));
    assert!(tab.tiled_panes.panes_to_hide_contains(survivor));

    tab.close_pane(editor, false, Some(0));
    tab.ensure_selectable_focus(client).unwrap();
    assert!(
        tab.has_selectable_panes(),
        "the tab still has a live destination"
    );
    let focused = tab
        .get_active_pane_id(client)
        .expect("retained tab has focus");
    assert!(
        tab.get_pane_with_id(focused).unwrap().selectable()
            && !tab.tiled_panes.panes_to_hide_contains(focused),
        "editor exit left focus on {focused:?}, fullscreen {:?}, survivor hidden {}",
        tab.tiled_panes.fullscreen_pane_id(),
        tab.tiled_panes.panes_to_hide_contains(survivor),
    );
    assert!(!tab.tiled_panes.fullscreen_is_active());
    assert_eq!(tab.get_active_pane_id(client), Some(survivor));
    tab.assert_invariants();
}

#[test]
fn buffered_hold_requeues_the_last_replacement_through_screen() {
    let mut tab = create_new_tab(Size { cols: 80, rows: 24 }, true);
    let id = TerminalId::new(1);
    let pane = PaneId::Terminal(id);
    let (tx, rx) = unbounded();
    tab.senders.to_screen = Some(SenderWithContext::new(tx));
    let (tx, pty) = unbounded();
    tab.senders.to_pty = Some(SenderWithContext::new(tx));
    let command = |name: &str| RunCommand {
        command: name.into(),
        ..Default::default()
    };
    assert!(!tab.rerun_terminal_pane_with_id(id, None));
    assert!(!tab.run_command_in_pane_with_id(id, command("first"), false));
    assert!(!tab.run_command_in_pane_with_id(id, command("last"), true));
    tab.is_pending = true;
    assert!(tab
        .hold_pane(pane, Some(0), false, command("old"))
        .is_none());
    tab.is_pending = false;
    tab.apply_buffered_instructions().unwrap();
    assert!(pty.try_iter().next().is_none());
    let queued: Vec<_> = rx.try_iter().collect();
    assert_eq!(queued.len(), 1);
    let (ScreenInstruction::RunCommandInPane(queued_id, replacement, keep), _) = &queued[0] else {
        panic!("expected queued restart");
    };
    assert_eq!(*queued_id, id);
    assert_eq!(*replacement, command("last"));
    assert!(*keep);
    assert!(tab.run_command_in_pane_with_id(*queued_id, replacement.clone(), *keep));
    assert_eq!(
        pty.try_iter()
            .filter(|(i, _)| matches!(i, PtyInstruction::ReRunCommandInPane(..)))
            .count(),
        1
    );
    assert!(!tab.rerun_terminal_pane_with_id(TerminalId::new(999), None));
    assert!(!tab.run_command_in_pane_with_id(TerminalId::new(999), command("missing"), false));
}

#[test]
fn navigation_recovers_only_the_lowest_eligible_parked_terminal() {
    for excluded in [
        "service",
        "held",
        "exited",
        "replacement",
        "stack",
        "plugin",
        "overlay",
    ] {
        let client = ClientId::new(1);
        let mut tab = create_new_tab(Size { cols: 80, rows: 24 }, true);
        let (sender, _receiver) = unbounded();
        tab.senders
            .replace_to_plugin(SenderWithContext::new(sender));
        let original = PaneId::Terminal(TerminalId::new(1));
        let excluded_id = match excluded {
            "plugin" => {
                let plugin = PaneId::Plugin(crate::PluginId::new(1));
                tab.new_pane(
                    plugin,
                    None,
                    None,
                    true,
                    false,
                    NewPanePlacement::default(),
                    Some(client),
                    None,
                )
                .unwrap();
                tab.close_pane(original, false, None);
                plugin
            },
            "overlay" => {
                let overlay = PaneId::Terminal(TerminalId::new(2));
                tab.new_pane(
                    overlay,
                    None,
                    None,
                    false,
                    false,
                    NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
                    Some(client),
                    None,
                )
                .unwrap();
                tab.hide_overlay_pane(overlay);
                tab.close_pane(original, false, None);
                overlay
            },
            _ => {
                match excluded {
                    "held" | "exited" => {
                        tab.hold_pane(original, Some(0), excluded == "held", RunCommand::default());
                    },
                    _ => {},
                }
                tab.suppress_pane(original, None);
                if excluded == "service" {
                    tab.suppressed_panes
                        .get_mut(&original)
                        .unwrap()
                        .pane
                        .set_selectable(false);
                }
                if excluded == "replacement" {
                    tab.suppressed_panes.get_mut(&original).unwrap().restored_by =
                        Some(crate::tab::StandIn {
                            pane_id: PaneId::Plugin(crate::PluginId::new(7)),
                            is_scrollback_editor: true,
                            lent: false,
                        });
                }
                if excluded == "stack" {
                    tab.stack_list_of_member.insert(original, 0);
                    tab.stack_lists.insert(
                        0,
                        crate::tab::StackList {
                            members: vec![original],
                            visible: original,
                        },
                    );
                }
                original
            },
        };
        assert!(
            !tab.recover_suppressed_terminal_for_navigation(client)
                .unwrap(),
            "{excluded}"
        );
        for id in [9, 8] {
            tab.new_pane(
                PaneId::Terminal(TerminalId::new(id)),
                None,
                None,
                true,
                false,
                NewPanePlacement::default(),
                Some(client),
                None,
            )
            .unwrap();
        }
        assert!(!tab.has_selectable_panes(), "{excluded}");
        assert!(
            tab.recover_suppressed_terminal_for_navigation(client)
                .unwrap_or_else(|error| panic!("{excluded}: {error:#}")),
            "{excluded}"
        );
        assert_eq!(
            tab.get_active_pane_id(client),
            Some(PaneId::Terminal(TerminalId::new(8))),
            "{excluded}"
        );
        assert!(tab
            .suppressed_panes
            .contains_key(&PaneId::Terminal(TerminalId::new(9))));
        assert!(tab.has_pane_with_pid(&excluded_id));
        assert!(!tab
            .recover_suppressed_terminal_for_navigation(client)
            .unwrap());
        if excluded != "overlay" {
            assert!(tab.suppressed_panes.contains_key(&excluded_id));
        } else {
            assert!(tab.pane_is_overlay(excluded_id));
        }
    }
}

#[test]
fn navigation_recovery_retains_the_terminal_when_resize_channel_is_closed() {
    let client = ClientId::new(1);
    let mut tab = create_new_tab(Size { cols: 80, rows: 24 }, true);
    let pane = PaneId::Terminal(TerminalId::new(1));
    tab.handle_pty_bytes(TerminalId::new(1), b"retained transcript".to_vec())
        .unwrap();
    tab.suppress_pane(pane, None);
    let (sender, receiver) = unbounded();
    tab.senders
        .replace_to_pty_writer(SenderWithContext::new(sender));
    tab.senders.should_silently_fail = false;
    drop(receiver);
    assert!(tab
        .recover_suppressed_terminal_for_navigation(client)
        .is_err());
    assert!(tab.floating_panes.panes_contain(&pane));
    assert!(tab
        .get_pane_with_id(pane)
        .unwrap()
        .dump_screen(true, None)
        .contains("retained transcript"));
}

#[test]
fn pending_tab_navigation_leaves_suppressed_work_untouched() {
    let client = ClientId::new(1);
    let mut tab = create_new_tab(Size { cols: 80, rows: 24 }, true);
    let pane = PaneId::Terminal(TerminalId::new(1));
    tab.suppress_pane(pane, None);
    tab.is_pending = true;
    assert!(!tab
        .recover_suppressed_terminal_for_navigation(client)
        .unwrap());
    assert!(tab.suppressed_panes.contains_key(&pane));
}

#[test]
fn navigation_leaves_editor_restoration_ownership_intact() {
    let client = ClientId::new(1);
    let mut tab = create_new_tab(Size { cols: 80, rows: 24 }, true);
    let original = PaneId::Terminal(TerminalId::new(1));
    let editor = PaneId::Terminal(TerminalId::new(2));
    tab.replace_active_pane_with_editor_pane(editor, client)
        .unwrap();
    tab.set_pane_selectable(editor, false);
    assert!(!tab
        .recover_suppressed_terminal_for_navigation(client)
        .unwrap());
    assert_eq!(
        tab.suppressed_panes[&original].restored_by.unwrap().pane_id,
        editor
    );
    tab.close_pane(editor, false, None);
    assert!(tab.has_non_suppressed_pane_with_pid(&original));
    assert!(!tab.suppressed_panes.contains_key(&original));
}

/// The merge-queue stack: an exited agent behind its `agent_exit` hook, and a held command
/// the hook ran in its own slot. The hook's link to the agent is an ordinary suppression;
/// the command only borrows the hook's slot.
struct LentStack {
    tab: Tab,
    client: ClientId,
    agent: PaneId,
    hook: PaneId,
    gate: PaneId,
    closed: Receiver<(PluginInstruction, yinz_types::diagnostics::ErrorContext)>,
    screen: Receiver<(ScreenInstruction, yinz_types::diagnostics::ErrorContext)>,
}

fn lent_stack() -> LentStack {
    let mut tab = create_new_tab(
        Size {
            cols: 121,
            rows: 20,
        },
        true,
    );
    let (sender, closed) = unbounded();
    tab.senders
        .replace_to_plugin(SenderWithContext::new(sender));
    let (screen_sender, screen) = unbounded();
    tab.senders.to_screen = Some(SenderWithContext::new(screen_sender));
    let client = ClientId::new(1);
    let agent = PaneId::Terminal(TerminalId::new(1));
    let hook = PaneId::Plugin(crate::PluginId::new(2));
    let gate = PaneId::Terminal(TerminalId::new(3));
    tab.hold_pane(agent, Some(0), false, RunCommand::default());
    tab.suppress_pane_and_replace_with_pid(agent, hook, ReplacedPane::Suppress, None, None, None)
        .unwrap();
    tab.suppress_pane_and_replace_with_pid(hook, gate, ReplacedPane::Lend, None, None, None)
        .unwrap();
    let held = RunCommand {
        hold_on_close: true,
        ..Default::default()
    };
    tab.hold_pane(gate, Some(1), false, held);
    tab.focus_pane_with_id(gate, false, false, client).unwrap();
    LentStack {
        tab,
        client,
        agent,
        hook,
        gate,
        closed,
        screen,
    }
}

fn announced_closures(
    receiver: &Receiver<(PluginInstruction, yinz_types::diagnostics::ErrorContext)>,
) -> Vec<PaneId> {
    receiver
        .try_iter()
        .flat_map(|(instruction, _)| match instruction {
            PluginInstruction::Update(events) => events
                .into_iter()
                .filter_map(|(_, _, event)| match event {
                    Event::PaneClosed(id) => Some(id),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            _ => vec![],
        })
        .collect()
}

#[test]
fn ctrl_c_on_a_held_command_in_a_lent_slot_restores_the_hook_over_its_agent() {
    let LentStack {
        mut tab,
        client,
        agent,
        hook,
        gate,
        closed,
        screen,
    } = lent_stack();
    tab.write_to_active_terminal(&None, vec![3], false, client, None)
        .unwrap();
    let closing = screen
        .try_iter()
        .find_map(|(instruction, _)| match instruction {
            ScreenInstruction::CloseFocusWithPaneId(id, completion) => Some((id, completion)),
            _ => None,
        });
    let Some((closing, completion)) = closing else {
        panic!("Ctrl-C on a held pane did not ask to close it");
    };
    assert_eq!(closing, gate);
    // What the screen does with `CloseFocusWithPaneId`.
    assert_eq!(tab.close_pane_explicitly(closing, completion), vec![gate]);
    assert!(!tab.has_pane_with_pid(&gate));
    assert!(
        tab.has_non_suppressed_pane_with_pid(&hook),
        "the hook is back in its slot"
    );
    assert_eq!(tab.get_active_pane_id(client), Some(hook));
    let behind = tab.suppressed_panes[&agent].restored_by.unwrap();
    assert_eq!(
        behind.pane_id, hook,
        "the agent is still suppressed behind it"
    );
    assert!(!behind.lent);
    assert_eq!(announced_closures(&closed), vec![gate]);
    tab.assert_invariants();
}

#[test]
fn explicit_close_keeps_the_hook_cascade_and_closes_a_whole_stack_from_its_bottom() {
    // (closed explicitly, expected to close, expected to survive in a slot)
    let cases = [
        ("gate", vec!["gate"], Some("hook")),
        ("agent", vec!["agent", "hook", "gate"], None),
        // A pane behind the lent slot still takes everything in front of it.
        ("hook", vec!["hook", "agent", "gate"], None),
    ];
    for (target, expected, survivor) in cases {
        let stack = lent_stack();
        let mut tab = stack.tab;
        let name = |id: PaneId| match id {
            id if id == stack.agent => "agent",
            id if id == stack.hook => "hook",
            id if id == stack.gate => "gate",
            _ => "other",
        };
        let id = |name: &str| match name {
            "agent" => stack.agent,
            "hook" => stack.hook,
            _ => stack.gate,
        };
        let mut closed: Vec<_> = tab
            .close_pane_explicitly(id(target), None)
            .into_iter()
            .map(name)
            .collect();
        closed.sort();
        let mut expected_sorted = expected.clone();
        expected_sorted.sort();
        assert_eq!(closed, expected_sorted, "closing {target}");
        for pane in ["agent", "hook", "gate"] {
            assert_eq!(
                tab.has_pane_with_pid(&id(pane)),
                !expected.contains(&pane),
                "{pane} after closing {target}"
            );
        }
        if let Some(survivor) = survivor {
            assert!(tab.has_non_suppressed_pane_with_pid(&id(survivor)));
        }
        tab.assert_invariants();
    }
    // With the gate gone, the user closing the hook closes the agent too, and so does the
    // builtin `agent_exit` Esc path (`CloseTerminalPane` on the agent, then `CloseSelf`).
    for esc_path in [false, true] {
        let LentStack {
            mut tab,
            agent,
            hook,
            gate,
            ..
        } = lent_stack();
        tab.close_pane_explicitly(gate, None);
        if esc_path {
            tab.close_pane_explicitly(agent, None);
            tab.close_pane(hook, false, None);
        } else {
            tab.close_pane_explicitly(hook, None);
        }
        for id in [agent, hook, gate] {
            assert!(!tab.has_pane_with_pid(&id), "esc_path={esc_path}");
        }
        tab.assert_invariants();
    }
}

#[test]
fn a_lent_slot_comes_back_on_every_kind_of_close() {
    for explicit in [false, true] {
        let mut tab = create_new_tab(
            Size {
                cols: 121,
                rows: 20,
            },
            true,
        );
        let (sender, _receiver) = unbounded();
        tab.senders
            .replace_to_plugin(SenderWithContext::new(sender));
        // A running agent a coordinator covered with its own UI: closing the UI must not
        // take the agent with it.
        let agent = PaneId::Terminal(TerminalId::new(1));
        let cover = PaneId::Plugin(crate::PluginId::new(2));
        tab.suppress_pane_and_replace_with_pid(agent, cover, ReplacedPane::Lend, None, None, None)
            .unwrap();
        assert!(tab.suppressed_panes[&agent].restored_by.unwrap().lent);
        if explicit {
            assert_eq!(tab.close_pane_explicitly(cover, None), vec![cover]);
        } else {
            tab.close_pane(cover, false, None);
        }
        assert!(
            tab.has_non_suppressed_pane_with_pid(&agent),
            "explicit={explicit}"
        );
        assert!(!tab.has_pane_with_pid(&cover));
        tab.assert_invariants();
    }
}

#[test]
fn opening_in_place_of_a_pane_without_a_slot_is_refused_before_anything_is_built() {
    for replaced in [
        ReplacedPane::Close,
        ReplacedPane::Suppress,
        ReplacedPane::Lend,
    ] {
        for missing in [false, true] {
            let mut tab = create_new_tab(
                Size {
                    cols: 121,
                    rows: 20,
                },
                true,
            );
            let (sender, _receiver) = unbounded();
            tab.senders
                .replace_to_plugin(SenderWithContext::new(sender));
            let (pty_sender, pty) = unbounded();
            tab.senders
                .replace_to_pty(SenderWithContext::new(pty_sender));
            let agent = PaneId::Terminal(TerminalId::new(1));
            let hook = PaneId::Plugin(crate::PluginId::new(2));
            let gate = PaneId::Terminal(TerminalId::new(3));
            tab.suppress_pane_and_replace_with_pid(
                agent,
                hook,
                ReplacedPane::Suppress,
                None,
                None,
                None,
            )
            .unwrap();
            // A hidden hook (`HideSelf`) asking to run its gate in its own place, or a pane
            // that is gone altogether.
            let target = if missing {
                PaneId::Terminal(TerminalId::new(99))
            } else {
                tab.suppress_pane(hook, None);
                hook
            };
            let before = tab.get_all_pane_ids();
            let error = tab
                .suppress_pane_and_replace_with_pid(target, gate, replaced, None, None, None)
                .unwrap_err();
            let expected = if missing {
                "not in this tab"
            } else {
                "suppressed"
            };
            assert!(format!("{error:#}").contains(expected), "{error:#}");
            assert_eq!(
                tab.get_all_pane_ids(),
                before,
                "{replaced:?} changed the tab"
            );
            assert!(!tab.has_pane_with_pid(&gate));
            assert!(
                pty.try_iter().next().is_none(),
                "the caller, which owns the gate's id, decides what to close"
            );
            // The editor in place of a pane has the same rule.
            let editor = PaneId::Terminal(TerminalId::new(4));
            assert!(tab.replace_pane_with_editor_pane(editor, target).is_err());
            assert!(!tab.has_pane_with_pid(&editor));
            tab.assert_invariants();
        }
    }
}

#[test]
fn moving_a_pane_into_a_pane_without_a_slot_parks_it_rather_than_dropping_it() {
    for suppress in [false, true] {
        let mut tab = create_new_tab(
            Size {
                cols: 121,
                rows: 20,
            },
            true,
        );
        let (sender, _receiver) = unbounded();
        tab.senders
            .replace_to_plugin(SenderWithContext::new(sender));
        let hidden = PaneId::Terminal(TerminalId::new(2));
        let moving = PaneId::Terminal(TerminalId::new(3));
        tab.vertical_split(hidden, None, None, ClientId::new(1), None, None)
            .unwrap();
        tab.vertical_split(moving, None, None, ClientId::new(1), None, None)
            .unwrap();
        tab.suppress_pane(hidden, None);
        let pane = tab.extract_pane(moving, true).unwrap();
        if suppress {
            tab.suppress_pane_and_replace_with_other_pane(hidden, pane, None);
        } else {
            tab.close_pane_and_replace_with_other_pane(hidden, pane, None);
        }
        assert!(tab.has_pane_with_pid(&moving), "suppress={suppress}");
        assert!(tab.has_pane_with_pid(&hidden));
        assert!(tab.suppressed_panes[&moving].restored_by.is_none());
        tab.assert_invariants();
    }
}

#[test]
fn a_plugin_opened_in_place_that_closes_the_replaced_pane_closes_its_process() {
    let mut tab = create_new_tab(
        Size {
            cols: 121,
            rows: 20,
        },
        true,
    );
    let (sender, _receiver) = unbounded();
    tab.senders
        .replace_to_plugin(SenderWithContext::new(sender));
    let (pty_sender, pty_receiver) = unbounded();
    tab.senders
        .replace_to_pty(SenderWithContext::new(pty_sender));
    let source = PaneId::Terminal(TerminalId::new(1));
    let plugin = PaneId::Plugin(crate::PluginId::new(2));
    tab.suppress_pane_and_replace_with_pid(source, plugin, ReplacedPane::Close, None, None, None)
        .unwrap();
    assert!(!tab.has_pane_with_pid(&source));
    assert!(tab.has_non_suppressed_pane_with_pid(&plugin));
    let closed: Vec<_> = pty_receiver
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PtyInstruction::ClosePane(id, _) => Some(id),
            _ => None,
        })
        .collect();
    assert_eq!(
        closed,
        vec![source],
        "the replaced terminal's process is closed"
    );
    tab.assert_invariants();
}
