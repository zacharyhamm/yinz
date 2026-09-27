use super::*;
use crate::tab::{AcceptedPane, ExtractedPanePlacement};
use zellij_bus::pty::PtyInstruction;
use zellij_types::data::Event;
use zellij_types::internal_ids::PluginId;
use zellij_types::keys::{BareKey, KeyWithModifier};
use zellij_types::pane_tabs::*;

fn layout() -> PaneTabsLayout {
    PaneTabsLayout {
        active: 0,
        tabs: ["First", "Second"]
            .map(|label| PaneTabDescriptor {
                label: label.into(),
                location: "zellij:fixture-plugin-for-tests".into(),
                initial_cwd: (label == "Second").then(|| std::path::PathBuf::from("/tmp/second")),
                configuration: BTreeMap::new(),
            })
            .to_vec(),
    }
}

#[test]
fn pane_tabs_retain_children_share_selection_and_isolate_input() {
    for placement in [
        NewPanePlacement::Floating(None),
        NewPanePlacement::NoPreference {
            borderless: Some(false),
        },
        NewPanePlacement::NoPreference {
            borderless: Some(true),
        },
    ] {
        let (mut tab, receiver) =
            create_new_tab_with_plugin_receiver(Size { cols: 80, rows: 24 }, ModeInfo::default());
        let client = ClientId::new(1);
        let first = PaneId::Plugin(PluginId::new(100));
        tab.new_pane(
            first,
            None,
            None,
            false,
            true,
            placement,
            Some(client),
            None,
        )
        .unwrap();
        let before = tab.get_pane_with_id(first).unwrap().get_content_rows();
        let borderless = tab.get_pane_with_id(first).unwrap().borderless();
        let info = tab.mount_pane_tabs(first, layout(), 0, client).unwrap();
        assert_eq!(
            tab.get_pane_with_id(first).unwrap().get_content_rows(),
            before - 1
        );
        assert!(receiver
            .try_iter()
            .all(|(instruction, _)| !matches!(instruction, PluginInstruction::LoadPaneTab { .. })));
        tab.select_pane_tab(info.id, 1, client).unwrap();
        let generation = receiver
            .try_iter()
            .find_map(|(instruction, _)| match instruction {
                PluginInstruction::LoadPaneTab {
                    generation,
                    index: 1,
                    descriptor,
                    tab_id,
                    ..
                } => {
                    assert_eq!(tab_id, tab.id);
                    assert_eq!(
                        descriptor.initial_cwd,
                        Some(std::path::PathBuf::from("/tmp/second"))
                    );
                    Some(generation)
                },
                _ => None,
            })
            .unwrap();
        let run = zellij_runtime::plugin_location::run_plugin_or_alias_from_url(
            "zellij:fixture-plugin-for-tests",
            &None,
            None,
            None,
        )
        .unwrap();
        let second = PluginId::new(101);
        tab.pane_tab_loaded(info.id, 1, generation, Ok((second, run)))
            .unwrap();
        assert_eq!(
            tab.get_pane_with_id(PaneId::Plugin(second))
                .unwrap()
                .borderless(),
            borderless
        );
        assert_eq!(
            tab.get_pane_with_id(PaneId::Plugin(second))
                .unwrap()
                .get_content_rows(),
            before - 1
        );
        assert!(tab.get_pane_info(first).unwrap().is_suppressed);
        assert_eq!(
            tab.pane_tabs_info(info.id).unwrap().pane_id,
            PaneId::Plugin(second)
        );
        tab.handle_plugin_bytes(PluginId::new(100), client, b"retained".to_vec())
            .unwrap();
        receiver.try_iter().for_each(drop);
        tab.write_to_pane_id(
            &Some(KeyWithModifier::new(BareKey::Char('x'))),
            b"x".to_vec(),
            false,
            first,
            Some(client),
            None,
        )
        .unwrap();
        assert!(receiver.try_iter().all(|(instruction, _)| !matches!(instruction, PluginInstruction::Update(events) if events.iter().any(|(_, _, event)| matches!(event, zellij_types::data::Event::Key(_))))));
        tab.select_pane_tab(info.id, 0, ClientId::new(2)).unwrap();
        assert_eq!(
            tab.get_pane_with_id(first).unwrap().borderless(),
            borderless
        );
        assert_eq!(
            tab.get_pane_with_id(first).unwrap().get_content_rows(),
            before - 1
        );
        assert_eq!(tab.pane_tabs_info(info.id).unwrap().active, 0);
        tab.focus_pane_tabs(info.id, true, client).unwrap();
        assert_eq!(tab.pane_tabs_info(info.id).unwrap().bar_focused, [client]);
        let mut destination = create_new_tab(Size { cols: 50, rows: 16 }, ModeInfo::default());
        let moved = tab.extract_pane(first, true).unwrap();
        assert!(tab.pane_tabs_info(info.id).is_none());
        assert!(matches!(
            destination.accept_extracted_pane(moved, ExtractedPanePlacement::Tiled),
            AcceptedPane::Accepted
        ));
        destination.select_pane_tab(info.id, 1, client).unwrap();
        assert!(destination.get_pane_info(first).unwrap().is_suppressed);
        assert_eq!(
            destination
                .get_pane_with_id(first)
                .unwrap()
                .get_content_rows(),
            destination
                .get_pane_with_id(PaneId::Plugin(second))
                .unwrap()
                .get_content_rows()
        );
        destination.select_pane_tab(info.id, 0, client).unwrap();
        let tab = &mut destination;
        tab.close_pane_tab_child(info.id, 1).unwrap();
        assert_eq!(
            tab.pane_tabs_info(info.id).unwrap().tabs[1].state,
            PaneTabLoadState::Unloaded
        );
        tab.close_pane_tabs(info.id).unwrap();
        assert!(tab.pane_tabs_info(info.id).is_none());
        assert!(tab.get_pane_with_id(first).is_none());
        assert!(tab.get_pane_with_id(PaneId::Plugin(second)).is_none());
    }
}

#[test]
fn pane_tabs_discard_stale_loads_and_keep_the_bar_usable_during_loading() {
    let (mut tab, receiver) =
        create_new_tab_with_plugin_receiver(Size { cols: 80, rows: 24 }, ModeInfo::default());
    let client = ClientId::new(1);
    let first = PaneId::Plugin(PluginId::new(100));
    tab.new_pane(
        first,
        None,
        None,
        false,
        true,
        NewPanePlacement::Floating(None),
        Some(client),
        None,
    )
    .unwrap();
    let info = tab.mount_pane_tabs(first, layout(), 0, client).unwrap();
    tab.select_pane_tab(info.id, 1, client).unwrap();
    tab.write_to_pane_id(
        &Some(KeyWithModifier::new(BareKey::Left)),
        vec![],
        false,
        first,
        Some(client),
        None,
    )
    .unwrap();
    assert_eq!(tab.pane_tabs_info(info.id).unwrap().active, 0);
    tab.close_pane_tab_child(info.id, 1).unwrap();
    let run = zellij_runtime::plugin_location::run_plugin_or_alias_from_url(
        "zellij:fixture-plugin-for-tests",
        &None,
        None,
        None,
    )
    .unwrap();
    tab.pane_tab_loaded(info.id, 1, 1, Ok((PluginId::new(101), run)))
        .unwrap();
    assert!(tab
        .get_pane_with_id(PaneId::Plugin(PluginId::new(101)))
        .is_none());
    assert!(receiver.try_iter().any(|(instruction, _)| matches!(instruction, PluginInstruction::Unload(id) if id == PluginId::new(101))));
}

#[test]
fn pane_tabs_overlay_moves_hidden_children_and_closes_during_loading() {
    use zellij_types::multipane::*;
    let client = ClientId::new(1);
    let (mut tab, receiver) =
        create_new_tab_with_plugin_receiver(Size { cols: 80, rows: 24 }, ModeInfo::default());
    let children = [
        PaneId::Plugin(PluginId::new(100)),
        PaneId::Plugin(PluginId::new(200)),
    ];
    for pane in children {
        tab.new_pane(
            pane,
            None,
            None,
            false,
            true,
            NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
            Some(client),
            None,
        )
        .unwrap();
    }
    let info = tab
        .mount_pane_tabs(children[0], layout(), 0, client)
        .unwrap();
    tab.create_multipane(
        children,
        MultipaneSplit {
            direction: MultipaneDirection::LeftRight,
            ratio: 5000,
        },
        0,
        OverlayPlacement::dismissible(None),
    )
    .unwrap();
    tab.select_pane_tab(info.id, 1, client).unwrap();
    tab.pane_tab_loaded(info.id, 1, 1, Err("missing plugin".into()))
        .unwrap();
    assert!(matches!(
        tab.pane_tabs_info(info.id).unwrap().tabs[1].state,
        PaneTabLoadState::Failed(_)
    ));
    tab.select_pane_tab(info.id, 1, client).unwrap();
    let transfer = tab.take_overlay_container(children[0]).unwrap();
    let mut destination = create_new_tab(Size { cols: 40, rows: 12 }, ModeInfo::default());
    destination.accept_overlay_container(transfer);
    assert!(tab.pane_tabs_info(info.id).is_none());
    destination.close_pane_tabs(info.id).unwrap();
    assert!(children
        .iter()
        .all(|pane| destination.get_pane_with_id(*pane).is_none()));
    let run = zellij_runtime::plugin_location::run_plugin_or_alias_from_url(
        "zellij:strider",
        &None,
        None,
        None,
    )
    .unwrap();
    // Completion arrives after closure: no pane is resurrected and the runtime is unloaded.
    tab.pane_tab_loaded(info.id, 1, 2, Ok((PluginId::new(101), run)))
        .unwrap();
    assert!(receiver.try_iter().any(|(instruction, _)| matches!(instruction, PluginInstruction::Unload(id) if id == PluginId::new(101))));
}

#[test]
fn pane_tabs_bar_keeps_active_label_reachable_at_every_width() {
    let info = PaneTabsInfo {
        id: PaneTabsId(1),
        pane_id: PaneId::Plugin(PluginId::new(1)),
        active: 1,
        bar_focused: vec![],
        tabs: layout()
            .tabs
            .into_iter()
            .map(|descriptor| PaneTabInfo {
                descriptor,
                plugin_id: None,
                state: PaneTabLoadState::Unloaded,
            })
            .collect(),
    };
    for width in 1..30 {
        let cells = zellij_panes::pane_tabs::bar_cells(&info, width);
        assert!(cells.len() <= width);
        assert!(cells.iter().any(|(_, index)| *index == 1));
    }
}

#[test]
fn pane_tabs_permanent_replacement_closes_children_and_discards_late_loads() {
    for placement in [
        NewPanePlacement::NoPreference {
            borderless: Some(false),
        },
        NewPanePlacement::Floating(None),
        NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
    ] {
        for replacement_kind in 0..3 {
            let (mut tab, receiver) = create_new_tab_with_plugin_receiver(
                Size { cols: 80, rows: 24 },
                ModeInfo::default(),
            );
            let (pty_sender, pty_receiver) = channels::unbounded();
            tab.senders
                .replace_to_pty(SenderWithContext::new(pty_sender));
            let client = ClientId::new(1);
            let first = PaneId::Plugin(PluginId::new(100));
            let second = PluginId::new(101);
            let late = PluginId::new(102);
            tab.new_pane(
                first,
                None,
                None,
                false,
                true,
                placement.clone(),
                Some(client),
                None,
            )
            .unwrap();
            let mut layout = layout();
            layout.tabs.push(layout.tabs[1].clone());
            let info = tab.mount_pane_tabs(first, layout, 0, client).unwrap();
            let run = zellij_runtime::plugin_location::run_plugin_or_alias_from_url(
                "zellij:fixture-plugin-for-tests",
                &None,
                None,
                None,
            )
            .unwrap();
            tab.select_pane_tab(info.id, 1, client).unwrap();
            tab.pane_tab_loaded(info.id, 1, 1, Ok((second, run.clone())))
                .unwrap();
            tab.select_pane_tab(info.id, 0, client).unwrap();
            tab.select_pane_tab(info.id, 2, client).unwrap();
            let geometry = tab.get_pane_with_id(first).unwrap().current_geom();
            let replacement = if replacement_kind == 0 {
                PaneId::Terminal(TerminalId::new(200))
            } else {
                PaneId::Plugin(PluginId::new(200))
            };
            pty_receiver.try_iter().for_each(drop);
            if replacement_kind == 2 {
                let pane = tab.build_pane(replacement, None, None, None).unwrap();
                tab.close_pane_and_replace_with_other_pane(first, pane, None);
            } else {
                tab.suppress_pane_and_replace_with_pid(first, replacement, true, None, None, None)
                    .unwrap();
            }
            assert!(tab.pane_tabs_info(info.id).is_none());
            assert!(tab.get_pane_with_id(first).is_none());
            assert!(tab.get_pane_with_id(PaneId::Plugin(second)).is_none());
            assert!(!tab.suppressed_panes.contains_key(&first));
            assert!(!tab.suppressed_panes.contains_key(&PaneId::Plugin(second)));
            assert_eq!(
                tab.get_pane_with_id(replacement).unwrap().current_geom(),
                geometry
            );
            match placement {
                NewPanePlacement::Floating(_) => {
                    assert!(tab.floating_panes.panes_contain(&replacement))
                },
                NewPanePlacement::Overlay(_) => {
                    assert!(tab.overlay_panes.panes_contain(&replacement))
                },
                _ => assert!(tab.tiled_panes.get_pane(replacement).is_some()),
            }
            let mut closed = pty_receiver
                .try_iter()
                .filter_map(|(instruction, _)| match instruction {
                    PtyInstruction::ClosePane(pane, _) => Some(pane),
                    _ => None,
                })
                .collect::<Vec<_>>();
            closed.sort();
            assert_eq!(closed, vec![first, PaneId::Plugin(second)]);
            tab.pane_tab_loaded(info.id, 2, 1, Ok((late, run))).unwrap();
            assert!(tab.get_pane_with_id(PaneId::Plugin(late)).is_none());
            assert!(receiver.try_iter().any(|(instruction, _)| matches!(instruction, PluginInstruction::Unload(id) if id == late)));
            assert!(tab.get_pane_with_id(replacement).is_some());
        }
    }
}

#[test]
fn pane_tabs_temporary_replacement_restores_composition() {
    let (mut tab, _receiver) =
        create_new_tab_with_plugin_receiver(Size { cols: 80, rows: 24 }, ModeInfo::default());
    let client = ClientId::new(1);
    let first = PaneId::Plugin(PluginId::new(100));
    let replacement = PaneId::Terminal(TerminalId::new(200));
    tab.new_pane(
        first,
        None,
        None,
        false,
        true,
        NewPanePlacement::Floating(None),
        Some(client),
        None,
    )
    .unwrap();
    let info = tab.mount_pane_tabs(first, layout(), 0, client).unwrap();
    tab.suppress_pane_and_replace_with_pid(first, replacement, false, None, None, None)
        .unwrap();
    assert!(tab.pane_tabs_info(info.id).is_some());
    tab.close_pane(replacement, false, None);
    assert!(tab.floating_panes.panes_contain(&first));
    assert!(tab.pane_tabs_info(info.id).is_some());
}

#[test]
fn pane_tabs_block_clientless_keys_and_paste_until_content_is_available() {
    let (mut tab, receiver) =
        create_new_tab_with_plugin_receiver(Size { cols: 80, rows: 24 }, ModeInfo::default());
    let client = ClientId::new(1);
    let first = PaneId::Plugin(PluginId::new(100));
    tab.new_pane(
        first,
        None,
        None,
        false,
        true,
        NewPanePlacement::Floating(None),
        Some(client),
        None,
    )
    .unwrap();
    let info = tab.mount_pane_tabs(first, layout(), 0, client).unwrap();
    tab.select_pane_tab(info.id, 1, client).unwrap();
    for state in 0..3 {
        if state == 1 {
            tab.pane_tab_loaded(info.id, 1, 1, Err("failed".into()))
                .unwrap();
        } else if state == 2 {
            tab.select_pane_tab(info.id, 0, client).unwrap();
        }
        let available = state == 2;
        receiver.try_iter().for_each(drop);
        tab.write_to_pane_id(
            &Some(KeyWithModifier::new(BareKey::Char('x'))),
            b"x".to_vec(),
            false,
            first,
            None,
            None,
        )
        .unwrap();
        tab.paste_to_pane_id(b"paste".to_vec(), first, None)
            .unwrap();
        let events = receiver
            .try_iter()
            .flat_map(|(instruction, _)| match instruction {
                PluginInstruction::Update(events) => events,
                _ => vec![],
            })
            .map(|(_, _, event)| event)
            .collect::<Vec<_>>();
        assert_eq!(
            events.iter().any(
                |event| matches!(event, Event::Key(key) if key.bare_key == BareKey::Char('x'))
            ),
            available
        );
        assert_eq!(
            events
                .iter()
                .any(|event| matches!(event, Event::PastedText(text) if text == "paste")),
            available
        );
    }
}

#[test]
fn pane_tabs_content_press_releases_only_pressing_clients_bar_focus() {
    for placement in [
        NewPanePlacement::NoPreference {
            borderless: Some(false),
        },
        NewPanePlacement::Floating(None),
        NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
    ] {
        let (mut tab, receiver) =
            create_new_tab_with_plugin_receiver(Size { cols: 80, rows: 24 }, ModeInfo::default());
        let client = ClientId::new(1);
        let other = ClientId::new(2);
        tab.add_client(other, None).unwrap();
        let first = PaneId::Plugin(PluginId::new(100));
        tab.new_pane(
            first,
            None,
            None,
            false,
            true,
            placement,
            Some(client),
            None,
        )
        .unwrap();
        tab.show_floating_panes();
        let info = tab.mount_pane_tabs(first, layout(), 0, client).unwrap();
        let pane = tab.get_pane_with_id(first).unwrap();
        let content = Position::new(pane.get_content_y() as i32, pane.get_content_x() as u16);
        let label = Position::new(content.line() as i32 - 1, content.column() as u16);
        for client in [client, other] {
            assert!(tab
                .pane_tabs_mouse(&MouseEvent::new_left_press_event(label), client)
                .unwrap());
        }
        let mut wheel = MouseEvent::new_buttonless_motion(content);
        wheel.event_type = zellij_types::mouse::MouseEventType::Press;
        wheel.wheel_down = true;
        for event in [
            MouseEvent::new_buttonless_motion(content),
            MouseEvent::new_left_release_event(content),
            wheel,
        ] {
            assert!(!tab.pane_tabs_mouse(&event, client).unwrap());
            assert_eq!(
                tab.pane_tabs_info(info.id).unwrap().bar_focused,
                [client, other]
            );
        }
        assert!(!tab
            .pane_tabs_mouse(&MouseEvent::new_left_press_event(content), client)
            .unwrap());
        assert_eq!(tab.pane_tabs_info(info.id).unwrap().bar_focused, [other]);
        receiver.try_iter().for_each(drop);
        for key in [BareKey::Right, BareKey::Enter] {
            tab.write_to_pane_id(
                &Some(KeyWithModifier::new(key)),
                vec![],
                false,
                first,
                Some(client),
                None,
            )
            .unwrap();
        }
        let keys = receiver
            .try_iter()
            .flat_map(|(instruction, _)| match instruction {
                PluginInstruction::Update(events) => events,
                _ => vec![],
            })
            .filter_map(|(_, _, event)| match event {
                Event::Key(key) => Some(key.bare_key),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(keys, [BareKey::Right, BareKey::Enter]);
        assert_eq!(tab.pane_tabs_info(info.id).unwrap().active, 0);
        tab.select_pane_tab(info.id, 1, other).unwrap();
        assert!(tab
            .pane_tabs_mouse(&MouseEvent::new_left_press_event(content), other)
            .unwrap());
        assert_eq!(tab.pane_tabs_info(info.id).unwrap().bar_focused, [other]);
    }
}
