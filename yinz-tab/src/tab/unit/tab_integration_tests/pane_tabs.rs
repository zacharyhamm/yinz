use super::*;
use crate::tab::{AcceptedPane, ExtractedPanePlacement};
use yinz_bus::pty::PtyInstruction;
use yinz_types::data::Event;
use yinz_types::input::layout::Run;
use yinz_types::internal_ids::PluginId;
use yinz_types::keys::{BareKey, KeyWithModifier};
use yinz_types::pane_tabs::*;

fn layout() -> PaneTabsLayout {
    PaneTabsLayout {
        active: 0,
        tabs: ["First", "Second"]
            .map(|label| PaneTabDescriptor {
                label: label.into(),
                location: "yinz:fixture-plugin-for-tests".into(),
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
        let run = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
            "yinz:fixture-plugin-for-tests",
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
        assert!(receiver.try_iter().all(|(instruction, _)| !matches!(instruction, PluginInstruction::Update(events) if events.iter().any(|(_, _, event)| matches!(event, yinz_types::data::Event::Key(_))))));
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
    assert_eq!(tab.pane_tabs_info(info.id).unwrap().active, 1);
    assert!(tab
        .pane_tabs_input(
            first,
            &Some(KeyWithModifier::new(BareKey::Enter)),
            Some(client)
        )
        .unwrap());
    assert_eq!(tab.pane_tabs_info(info.id).unwrap().active, 0);
    tab.close_pane_tab_child(info.id, 1).unwrap();
    let run = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "yinz:fixture-plugin-for-tests",
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
    use yinz_types::multipane::*;
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
    let run = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "yinz:strider",
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
        overlay_group: false,
        id: PaneTabsId(1),
        pane_id: PaneId::Plugin(PluginId::new(1)),
        active: 1,
        bar_focused: vec![],
        tabs: layout()
            .tabs
            .into_iter()
            .map(|descriptor| PaneTabInfo {
                terminal_id: None,
                descriptor,
                plugin_id: None,
                state: PaneTabLoadState::Unloaded,
            })
            .collect(),
    };
    for width in 0..30 {
        let cells = yinz_panes::pane_tabs::bar_cells(&info, Default::default(), false, width);
        assert!(cells.iter().map(|cell| cell.width).sum::<usize>() <= width);
        if width > 0 {
            assert!(cells
                .iter()
                .any(|cell| cell.target == yinz_types::tab_bar::TabTarget::Tab(1)));
        }
        let rendered = yinz_panes::pane_tabs::render_bar(
            &info,
            Default::default(),
            false,
            width,
            &Style::default(),
        );
        assert_eq!(rendered.iter().map(|ch| ch.width()).sum::<usize>(), width);
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
            let run = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
                "yinz:fixture-plugin-for-tests",
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
                tab.suppress_pane_and_replace_with_pid(
                    first,
                    replacement,
                    yinz_bus::pty::ReplacedPane::Close,
                    None,
                    None,
                    None,
                )
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
    tab.suppress_pane_and_replace_with_pid(
        first,
        replacement,
        yinz_bus::pty::ReplacedPane::Suppress,
        None,
        None,
        None,
    )
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
            assert!(!tab
                .pane_tabs_info(info.id)
                .unwrap()
                .bar_focused
                .contains(&client));
            tab.focus_pane_tabs(info.id, true, client).unwrap();
        }
        let mut wheel = MouseEvent::new_buttonless_motion(content);
        wheel.event_type = yinz_types::mouse::MouseEventType::Press;
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

#[test]
fn pane_tabs_highlight_is_per_client_and_only_enter_activates() {
    let (mut tab, receiver) =
        create_new_tab_with_plugin_receiver(Size { cols: 80, rows: 24 }, ModeInfo::default());
    let client = ClientId::new(1);
    let other = ClientId::new(2);
    tab.add_client(other, None).unwrap();
    let pane = PaneId::Plugin(PluginId::new(100));
    tab.new_pane(
        pane,
        None,
        None,
        false,
        true,
        NewPanePlacement::NoPreference {
            borderless: Some(false),
        },
        Some(client),
        None,
    )
    .unwrap();
    let info = tab.mount_pane_tabs(pane, layout(), 0, client).unwrap();
    for client in [client, other] {
        tab.focus_pane_tabs(info.id, true, client).unwrap();
    }
    receiver.try_iter().for_each(drop);
    for key in [
        BareKey::Right,
        BareKey::Right,
        BareKey::Char('j'),
        BareKey::Down,
        BareKey::Char('x'),
    ] {
        assert!(tab
            .pane_tabs_input(pane, &Some(KeyWithModifier::new(key)), Some(client))
            .unwrap());
    }
    assert!(tab
        .pane_tabs_input(
            pane,
            &Some(KeyWithModifier::new(BareKey::Char('x')).with_ctrl_modifier()),
            Some(client)
        )
        .unwrap());
    assert!(tab.pane_tabs_input(pane, &None, Some(client)).unwrap());
    tab.paste_to_active_terminal(b"ignored paste".to_vec(), client, None)
        .unwrap();
    assert_eq!(tab.pane_tabs[&info.id].views[&client].highlight, 1);
    assert_eq!(tab.pane_tabs[&info.id].views[&other].highlight, 0);
    assert_eq!(tab.pane_tabs_info(info.id).unwrap().active, 0);
    assert!(receiver.try_iter().all(|(instruction, _)| !matches!(
        instruction,
        PluginInstruction::LoadPaneTab { .. } | PluginInstruction::Update(_)
    )));
    for cancel in [BareKey::Esc, BareKey::Tab] {
        assert!(tab
            .pane_tabs_input(pane, &Some(KeyWithModifier::new(cancel)), Some(client))
            .unwrap());
        assert_eq!(tab.pane_tabs_info(info.id).unwrap().active, 0);
        assert_eq!(tab.pane_tabs_info(info.id).unwrap().bar_focused, [other]);
        tab.focus_pane_tabs(info.id, true, client).unwrap();
        assert_eq!(tab.pane_tabs[&info.id].views[&client].highlight, 0);
        tab.pane_tabs_input(
            pane,
            &Some(KeyWithModifier::new(BareKey::Right)),
            Some(client),
        )
        .unwrap();
    }
    assert!(!tab
        .pane_tabs_input(
            pane,
            &Some(KeyWithModifier::new(BareKey::Tab).with_shift_modifier()),
            Some(client)
        )
        .unwrap());
    tab.pane_tabs_input(
        pane,
        &Some(KeyWithModifier::new(BareKey::Enter)),
        Some(client),
    )
    .unwrap();
    assert_eq!(tab.pane_tabs_info(info.id).unwrap().active, 1);
    assert_eq!(tab.pane_tabs_info(info.id).unwrap().bar_focused, [other]);
    assert_eq!(tab.pane_tabs[&info.id].views[&other].highlight, 0);
    assert_eq!(
        receiver
            .try_iter()
            .filter(|(instruction, _)| matches!(
                instruction,
                PluginInstruction::LoadPaneTab { index: 1, .. }
            ))
            .count(),
        1
    );
    tab.pane_tab_loaded(info.id, 1, 1, Err("missing plugin".into()))
        .unwrap();
    tab.pane_tabs_input(
        pane,
        &Some(KeyWithModifier::new(BareKey::Char('r'))),
        Some(other),
    )
    .unwrap();
    assert_eq!(
        tab.pane_tabs_info(info.id).unwrap().tabs[1].state,
        PaneTabLoadState::Loading
    );
    assert!(receiver.try_iter().any(|(instruction, _)| matches!(
        instruction,
        PluginInstruction::LoadPaneTab {
            index: 1,
            generation: 3,
            ..
        }
    )));
    tab.remove_client(other);
    assert!(tab.pane_tabs_info(info.id).unwrap().bar_focused.is_empty());
    assert!(!tab.pane_tabs[&info.id].views.contains_key(&other));
}

#[test]
fn pane_tabs_mouse_uses_render_geometry_and_overflow_does_not_activate() {
    use yinz_panes::pane_tabs::bar_cells;
    use yinz_types::tab_bar::TabTarget;
    let (mut tab, receiver) =
        create_new_tab_with_plugin_receiver(Size { cols: 16, rows: 12 }, ModeInfo::default());
    let client = ClientId::new(1);
    let other = ClientId::new(2);
    tab.add_client(other, None).unwrap();
    let pane = PaneId::Plugin(PluginId::new(100));
    tab.new_pane(
        pane,
        None,
        None,
        false,
        true,
        NewPanePlacement::NoPreference {
            borderless: Some(false),
        },
        Some(client),
        None,
    )
    .unwrap();
    let mut unicode_layout = layout();
    unicode_layout.tabs[0].label = "第一".into();
    unicode_layout.tabs[1].label = "界e\u{301}👩‍💻".into();
    let info = tab
        .mount_pane_tabs(pane, unicode_layout, 0, client)
        .unwrap();
    let content = tab.get_pane_with_id(pane).unwrap();
    let x = content.get_content_x();
    let y = content.get_content_y() - 1;
    let width = content.get_content_columns();
    let click =
        |column| MouseEvent::new_left_press_event(Position::new(y as i32, (x + column) as u16));
    assert!(!tab
        .pane_tabs_mouse(
            &MouseEvent::new_left_press_event(Position::new(y as i32, (x - 1) as u16)),
            client
        )
        .unwrap());
    assert!(!tab.pane_tabs_mouse(&click(width), client).unwrap());
    let cells = bar_cells(&info, Default::default(), false, width);
    let arrow = cells
        .iter()
        .find(|cell| cell.target == TabTarget::Reveal(1))
        .unwrap();
    receiver.try_iter().for_each(drop);
    // Both columns of an arrow reveal the same window.
    for column in arrow.x..arrow.x + arrow.width {
        tab.focus_pane_tabs(info.id, true, client).unwrap();
        assert!(tab.pane_tabs_mouse(&click(column), client).unwrap());
        assert_eq!(tab.pane_tabs_info(info.id).unwrap().active, 0);
        assert_eq!(tab.pane_tabs[&info.id].views[&client].reveal, Some(1));
        assert!(!tab.pane_tabs[&info.id].views.contains_key(&other));
        assert!(tab.pane_tabs_info(info.id).unwrap().bar_focused.is_empty());
    }
    assert!(receiver
        .try_iter()
        .all(|(instruction, _)| !matches!(instruction, PluginInstruction::LoadPaneTab { .. })));
    let cells = bar_cells(&info, tab.pane_tabs[&info.id].views[&client], false, width);
    let target = cells
        .iter()
        .find(|cell| cell.target == TabTarget::Tab(1))
        .unwrap();
    let end = cells.last().unwrap().x + cells.last().unwrap().width;
    if end < width {
        tab.pane_tabs_mouse(&click(end), client).unwrap();
        assert_eq!(tab.pane_tabs_info(info.id).unwrap().active, 0);
        assert_eq!(tab.pane_tabs[&info.id].views[&client].reveal, Some(1));
    }
    tab.pane_tabs_mouse(&click(arrow.x), other).unwrap();
    tab.pane_tabs_mouse(&click(target.x + target.width - 1), client)
        .unwrap();
    assert_eq!(tab.pane_tabs_info(info.id).unwrap().active, 1);
    assert!(tab.pane_tabs_info(info.id).unwrap().bar_focused.is_empty());
    assert_eq!(tab.pane_tabs[&info.id].views[&other].reveal, Some(1));
    tab.remove_client(other);
    assert!(!tab.pane_tabs[&info.id].views.contains_key(&other));
}

#[test]
fn pane_tabs_render_active_inactive_and_independent_keyboard_highlight() {
    use yinz_panes::panes::terminal_character::AnsiCode;
    use yinz_types::colors::PaletteColor;
    let (mut tab, _) =
        create_new_tab_with_plugin_receiver(Size { cols: 80, rows: 24 }, ModeInfo::default());
    let client = ClientId::new(1);
    let plugin = PluginId::new(100);
    let pane = PaneId::Plugin(plugin);
    tab.new_pane(
        pane,
        None,
        None,
        false,
        true,
        NewPanePlacement::NoPreference {
            borderless: Some(false),
        },
        Some(client),
        None,
    )
    .unwrap();
    tab.handle_plugin_bytes(plugin, client, b"content".to_vec())
        .unwrap();
    let info = tab.mount_pane_tabs(pane, layout(), 0, client).unwrap();
    for highlight in [None, Some(0), Some(1)] {
        if let Some(highlight) = highlight {
            tab.focus_pane_tabs(info.id, true, client).unwrap();
            if highlight == 1 {
                tab.pane_tabs_input(
                    pane,
                    &Some(KeyWithModifier::new(BareKey::Right)),
                    Some(client),
                )
                .unwrap();
            }
        }
        for [base, background, emphasis_2, selected_base, selected_background] in [
            [
                (30, 31, 32),
                (240, 241, 242),
                (10, 70, 80),
                (250, 251, 252),
                (50, 60, 70),
            ],
            [
                (220, 221, 222),
                (20, 21, 22),
                (100, 200, 210),
                (10, 11, 12),
                (150, 160, 170),
            ],
        ] {
            let mut theme = Style::default().colors;
            theme.text_unselected.base = PaletteColor::Rgb(base);
            theme.text_unselected.background = PaletteColor::Rgb(background);
            theme.text_unselected.emphasis_2 = PaletteColor::Rgb(emphasis_2);
            theme.text_selected.base = PaletteColor::Rgb(selected_base);
            theme.text_selected.background = PaletteColor::Rgb(selected_background);
            tab.update_theme(theme);

            let pane = tab.get_pane_with_id_mut(pane).unwrap();
            let y = pane.get_content_y() - 1;
            let rendered = pane.render(Some(client)).unwrap().unwrap();
            let bar = rendered.0.iter().find(|chunk| chunk.y == y).unwrap();
            let text: String = bar
                .terminal_characters
                .iter()
                .map(|ch| ch.character)
                .collect();
            assert!(text.starts_with(" First │ Second │"));
            assert_eq!(bar.terminal_characters.len(), pane.get_content_columns());
            assert!(bar.terminal_characters[17..]
                .iter()
                .all(|ch| ch.character == ' '));

            for (range, index, foreground, bold) in [
                (0..8, Some(0), emphasis_2, AnsiCode::On),
                (8..17, Some(1), base, AnsiCode::Reset),
                (17..pane.get_content_columns(), None, base, AnsiCode::Reset),
            ] {
                let focused = index.is_some() && index == highlight;
                for ch in &bar.terminal_characters[range] {
                    assert_eq!(
                        ch.styles.foreground,
                        Some(AnsiCode::RgbCode(if focused {
                            selected_base
                        } else {
                            foreground
                        }))
                    );
                    assert_eq!(
                        ch.styles.background,
                        Some(AnsiCode::RgbCode(if focused {
                            selected_background
                        } else {
                            background
                        }))
                    );
                    assert_eq!(ch.styles.bold, Some(bold));
                    assert_eq!(ch.styles.dim, Some(AnsiCode::Reset));
                }
            }
        }
    }
}

#[test]
fn navigation_recovery_leaves_hidden_pane_tab_children_untouched() {
    let (mut tab, receiver) =
        create_new_tab_with_plugin_receiver(Size { cols: 80, rows: 24 }, ModeInfo::default());
    let client = ClientId::new(1);
    let parent = PaneId::Plugin(PluginId::new(100));
    tab.new_pane(
        parent,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client),
        None,
    )
    .unwrap();
    let info = tab.mount_pane_tabs(parent, layout(), 0, client).unwrap();
    tab.select_pane_tab(info.id, 1, client).unwrap();
    let generation = receiver
        .try_iter()
        .find_map(|(instruction, _)| match instruction {
            PluginInstruction::LoadPaneTab {
                generation,
                index: 1,
                ..
            } => Some(generation),
            _ => None,
        })
        .unwrap();
    let run = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "yinz:fixture-plugin-for-tests",
        &None,
        None,
        None,
    )
    .unwrap();
    let child = PluginId::new(101);
    tab.pane_tab_loaded(info.id, 1, generation, Ok((child, run)))
        .unwrap();
    tab.select_pane_tab(info.id, 0, client).unwrap();
    tab.set_pane_selectable(parent, false);
    let terminal = PaneId::Terminal(TerminalId::new(1));
    tab.suppress_pane(terminal, None);
    let before = tab.pane_tabs_info(info.id).unwrap();
    assert!(tab
        .recover_suppressed_terminal_for_navigation(client)
        .unwrap());
    assert_eq!(tab.get_active_pane_id(client), Some(terminal));
    assert_eq!(tab.pane_tabs_info(info.id).unwrap(), before);
    assert_eq!(tab.pane_tabs_of(PaneId::Plugin(child)), Some(info.id));
    assert!(tab.suppressed_panes.contains_key(&PaneId::Plugin(child)));
}

#[test]
fn clients_can_return_to_conversation_while_panel_load_is_pending_or_failed() {
    for failed in [false, true] {
        for recovering in [ClientId::new(1), ClientId::new(2)] {
            let (mut tab, receiver) = create_new_tab_with_plugin_receiver(
                Size { cols: 80, rows: 24 },
                ModeInfo::default(),
            );
            let client = ClientId::new(1);
            let other = ClientId::new(2);
            let terminal = PaneId::Terminal(TerminalId::new(100));
            tab.new_pane(
                terminal,
                None,
                None,
                false,
                true,
                NewPanePlacement::Floating(None),
                Some(client),
                None,
            )
            .unwrap();
            let info = tab.mount_pane_tabs(terminal, layout(), 0, client).unwrap();
            tab.select_pane_tab(info.id, 1, client).unwrap();
            assert_eq!(
                tab.pane_tabs_info(info.id).unwrap().tabs[1].state,
                PaneTabLoadState::Loading
            );
            if failed {
                let generation = receiver
                    .try_iter()
                    .find_map(|(instruction, _)| match instruction {
                        PluginInstruction::LoadPaneTab { generation, .. } => Some(generation),
                        _ => None,
                    })
                    .unwrap();
                tab.pane_tab_loaded(info.id, 1, generation, Err("load failed".into()))
                    .unwrap();
            }
            tab.connected_clients.borrow_mut().insert(other);
            tab.focus_pane_with_id(terminal, false, false, other)
                .unwrap();
            assert_eq!(tab.pane_tabs_info(info.id).unwrap().active, 1);
            assert_eq!(tab.pane_tabs_info(info.id).unwrap().pane_id, terminal);
            tab.select_pane_tab(info.id, 0, recovering).unwrap();
            assert_eq!(tab.pane_tabs_info(info.id).unwrap().active, 0);
            for client in [client, other] {
                assert_eq!(tab.get_active_pane_id(client), Some(terminal));
            }
        }
    }
}

#[test]
fn terminal_and_panel_tabs_retain_terminal_through_switch_transfer_and_close() {
    let (mut tab, receiver) =
        create_new_tab_with_plugin_receiver(Size { cols: 80, rows: 24 }, ModeInfo::default());
    let client = ClientId::new(1);
    let terminal = PaneId::Terminal(TerminalId::new(100));
    tab.new_pane(
        terminal,
        None,
        None,
        false,
        true,
        NewPanePlacement::Floating(None),
        Some(client),
        None,
    )
    .unwrap();
    let before = tab.get_pane_with_id(terminal).unwrap().get_content_rows();
    let info = tab.mount_pane_tabs(terminal, layout(), 0, client).unwrap();
    assert_eq!(
        tab.get_pane_with_id(terminal).unwrap().get_content_rows(),
        before - 1
    );
    let other = ClientId::new(2);
    tab.connected_clients.borrow_mut().insert(other);
    tab.focus_pane_with_id(terminal, false, false, other)
        .unwrap();
    tab.select_pane_tab(info.id, 1, client).unwrap();
    assert_eq!(
        tab.pane_tabs_info(info.id).unwrap().active,
        0,
        "another client owns Conversation"
    );
    tab.connected_clients.borrow_mut().remove(&other);
    tab.select_pane_tab(info.id, 1, client).unwrap();
    let generation = receiver
        .try_iter()
        .find_map(|(instruction, _)| match instruction {
            PluginInstruction::LoadPaneTab { generation, .. } => Some(generation),
            _ => None,
        })
        .unwrap();
    tab.pane_tab_loaded(info.id, 1, generation, Err("load failed".into()))
        .unwrap();
    tab.select_pane_tab(info.id, 0, client).unwrap();
    assert!(tab.get_pane_with_id(terminal).is_some());
    tab.select_pane_tab(info.id, 1, client).unwrap();
    let generation = receiver
        .try_iter()
        .find_map(|(instruction, _)| match instruction {
            PluginInstruction::LoadPaneTab { generation, .. } => Some(generation),
            _ => None,
        })
        .unwrap();
    let panel = PluginId::new(101);
    let run = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "yinz:fixture-plugin-for-tests",
        &None,
        None,
        None,
    )
    .unwrap();
    tab.pane_tab_loaded(info.id, 1, generation, Ok((panel, run)))
        .unwrap();
    assert!(tab.get_pane_info(terminal).unwrap().is_suppressed);
    assert!(
        !tab.pane_tabs_input(terminal, &None, None).unwrap(),
        "host delivery reaches inactive terminal"
    );
    assert!(
        tab.pane_tabs_input(terminal, &None, Some(client)).unwrap(),
        "client input cannot reach inactive terminal"
    );
    let mut destination = create_new_tab(Size { cols: 50, rows: 16 }, ModeInfo::default());
    let moved = tab.extract_pane(PaneId::Plugin(panel), true).unwrap();
    assert!(matches!(
        destination.accept_extracted_pane(moved, ExtractedPanePlacement::Tiled),
        AcceptedPane::Accepted
    ));
    assert!(destination.get_pane_with_id(terminal).is_some());
    destination.select_pane_tab(info.id, 0, client).unwrap();
    assert_eq!(
        destination.pane_tabs_info(info.id).unwrap().pane_id,
        terminal
    );
    destination.close_pane_tabs(info.id).unwrap();
    assert!(destination.get_pane_with_id(terminal).is_none());
    assert!(destination
        .get_pane_with_id(PaneId::Plugin(panel))
        .is_none());
}

#[test]
fn panel_dismissal_finishes_in_the_focus_transition_before_reveal() {
    for companion_active in [false, true] {
        for focus_path in 0..6 {
            let (mut tab, _receiver) = create_new_tab_with_plugin_receiver(
                Size { cols: 80, rows: 24 },
                ModeInfo::default(),
            );
            let client = ClientId::new(1);
            let panel = PaneId::Plugin(PluginId::new(100));
            tab.new_pane(
                panel,
                None,
                None,
                false,
                true,
                NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
                Some(client),
                None,
            )
            .unwrap();
            let mut panel_layout = layout();
            panel_layout.tabs[usize::from(companion_active)].location = "yinz:agents".into();
            let info = tab.mount_pane_tabs(panel, panel_layout, 0, client).unwrap();
            tab.mark_overlay_tab_group(info.id).unwrap();
            assert_eq!(tab.get_active_pane_id(client), Some(panel));
            if focus_path >= 3 {
                let floating = PaneId::Terminal(TerminalId::new(3));
                tab.new_floating_pane(floating, None, None, false, false, None, None)
                    .unwrap();
                tab.focus_pane_with_id(floating, false, false, client)
                    .unwrap();
                assert_eq!(tab.get_active_pane_id(client), Some(floating));
                assert!(!tab.overlay_panes.is_hidden(&panel));
                if focus_path % 3 != 1 {
                    tab.hide_floating_panes();
                }
            }
            match focus_path % 3 {
                0 => tab.release_overlay_focus_to_lower_layer(client),
                1 => tab
                    .focus_pane_with_id(PaneId::Terminal(TerminalId::new(1)), false, false, client)
                    .unwrap(),
                _ => tab
                    .new_pane(
                        PaneId::Terminal(TerminalId::new(2)),
                        None,
                        None,
                        false,
                        true,
                        NewPanePlacement::default(),
                        Some(client),
                        None,
                    )
                    .unwrap(),
            }
            assert!(tab.overlay_panes.is_hidden(&panel), "panel must be hidden before focus completes: companion={companion_active}, path={focus_path}");
            tab.focus_pane_with_id(panel, true, false, client).unwrap();
            assert!(!tab.overlay_panes.is_hidden(&panel));
            assert_eq!(tab.get_active_pane_id(client), Some(panel));
        }
    }
}

#[test]
fn panel_dismissal_preserves_another_clients_panel_during_tiled_navigation() {
    for companion_active in [false, true] {
        let (mut tab, _receiver) =
            create_new_tab_with_plugin_receiver(Size { cols: 80, rows: 24 }, ModeInfo::default());
        let client = ClientId::new(1);
        let other = ClientId::new(2);
        let tiled = PaneId::Terminal(TerminalId::new(1));
        let next = PaneId::Terminal(TerminalId::new(2));
        tab.new_pane(
            next,
            None,
            None,
            false,
            false,
            NewPanePlacement::default(),
            Some(client),
            None,
        )
        .unwrap();
        tab.connected_clients.borrow_mut().insert(other);
        let panel = add_agents_panel(&mut tab, companion_active, other);
        assert_eq!(tab.get_active_pane_id(client), Some(tiled));
        assert_eq!(tab.get_active_pane_id(other), Some(panel));

        tab.focus_next_pane(client);

        assert_eq!(tab.get_active_pane_id(client), Some(next));
        assert!(!tab.overlay_panes.is_hidden(&panel));
        assert_eq!(tab.get_active_pane_id(other), Some(panel));
    }
}

#[test]
fn panel_dismissal_finishes_when_floating_panes_are_hidden_closed_or_extracted() {
    for companion_active in [false, true] {
        for action in ["hide", "close", "extract"] {
            let (mut tab, _receiver) = create_new_tab_with_plugin_receiver(
                Size { cols: 80, rows: 24 },
                ModeInfo::default(),
            );
            let client = ClientId::new(1);
            let other = ClientId::new(2);
            let tiled = PaneId::Terminal(TerminalId::new(1));
            let floating = PaneId::Terminal(TerminalId::new(2));
            let panel = add_agents_panel(&mut tab, companion_active, client);
            tab.new_floating_pane(floating, None, None, false, false, None, None)
                .unwrap();
            tab.focus_pane_with_id(floating, false, false, client)
                .unwrap();
            let unrelated = PaneId::Plugin(PluginId::new(101));
            tab.connected_clients.borrow_mut().insert(other);
            tab.new_pane(
                unrelated,
                None,
                None,
                false,
                true,
                NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
                Some(other),
                None,
            )
            .unwrap();
            assert_eq!(tab.get_active_pane_id(client), Some(floating));
            assert!(!tab.overlay_panes.is_hidden(&panel));
            assert_eq!(tab.get_active_pane_id(other), Some(unrelated));

            match action {
                "hide" => tab.hide_floating_panes(),
                "close" => tab.close_pane(floating, false, None),
                "extract" => assert!(tab.extract_pane(floating, true).is_some()),
                _ => unreachable!(),
            }

            assert!(!tab.floating_panes.panes_are_shown());
            assert_eq!(tab.get_active_pane_id(client), Some(tiled));
            assert!(
                tab.overlay_panes.is_hidden(&panel),
                "companion={companion_active}, action={action}"
            );
            assert!(!tab.overlay_panes.is_hidden(&unrelated));
            assert_eq!(tab.get_active_pane_id(other), Some(unrelated));
            tab.focus_pane_with_id(panel, true, false, client).unwrap();
            assert!(!tab.overlay_panes.is_hidden(&panel));
            assert_eq!(tab.get_active_pane_id(client), Some(panel));
            tab.hide_floating_panes();
            assert!(!tab.overlay_panes.is_hidden(&panel));
            assert_eq!(tab.get_active_pane_id(client), Some(panel));
        }
    }
}

#[test]
fn panel_dismissal_preserves_overlay_focus_when_hiding_floating_panes() {
    for companion_active in [false, true] {
        for focused_overlay in ["panel", "modal", "unrelated"] {
            let (mut tab, _receiver) = create_new_tab_with_plugin_receiver(
                Size { cols: 80, rows: 24 },
                ModeInfo::default(),
            );
            let client = ClientId::new(1);
            let floating = PaneId::Terminal(TerminalId::new(2));
            let panel = add_agents_panel(&mut tab, companion_active, client);
            tab.new_floating_pane(floating, None, None, false, false, None, None)
                .unwrap();
            tab.focus_pane_with_id(floating, false, false, client)
                .unwrap();
            let focused = if focused_overlay == "panel" {
                tab.focus_pane_with_id(panel, true, false, client).unwrap();
                panel
            } else {
                let overlay = PaneId::Plugin(PluginId::new(101));
                tab.new_pane(
                    overlay,
                    None,
                    None,
                    false,
                    true,
                    NewPanePlacement::Overlay(if focused_overlay == "modal" {
                        OverlayPlacement::modal(None)
                    } else {
                        OverlayPlacement::dismissible(None)
                    }),
                    Some(client),
                    None,
                )
                .unwrap();
                overlay
            };
            assert!(tab.floating_panes.panes_are_shown());
            assert_eq!(tab.get_active_pane_id(client), Some(focused));

            tab.hide_floating_panes();

            assert!(!tab.floating_panes.panes_are_shown());
            assert_eq!(tab.get_active_pane_id(client), Some(focused));
            assert!(!tab.overlay_panes.is_hidden(&focused));
            assert!(!tab.overlay_panes.is_hidden(&panel));
        }
    }
}

#[test]
fn panel_dismissal_waits_for_the_last_client_on_any_container_member() {
    use yinz_types::multipane::*;

    for companion_active in [false, true] {
        for sibling_focused in [false, true] {
            for focus_path in 0..4 {
                let (mut tab, _receiver) = create_new_tab_with_plugin_receiver(
                    Size { cols: 80, rows: 24 },
                    ModeInfo::default(),
                );
                let client = ClientId::new(1);
                let other = ClientId::new(2);
                let tiled = PaneId::Terminal(TerminalId::new(1));
                let panel = add_agents_panel(&mut tab, companion_active, client);
                let protected = if sibling_focused {
                    let sibling = PaneId::Plugin(PluginId::new(101));
                    tab.new_pane(
                        sibling,
                        None,
                        None,
                        false,
                        false,
                        NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
                        Some(client),
                        None,
                    )
                    .unwrap();
                    tab.create_multipane(
                        [panel, sibling],
                        MultipaneSplit {
                            direction: MultipaneDirection::LeftRight,
                            ratio: 5000,
                        },
                        0,
                        OverlayPlacement::dismissible(None),
                    )
                    .unwrap();
                    sibling
                } else {
                    panel
                };
                tab.connected_clients.borrow_mut().insert(other);
                tab.focus_pane_with_id(protected, true, false, other)
                    .unwrap();
                assert_eq!(tab.get_active_pane_id(client), Some(panel));
                assert_eq!(tab.get_active_pane_id(other), Some(protected));

                match focus_path {
                    0 => tab.release_overlay_focus_to_lower_layer(client),
                    1 => tab.focus_pane_with_id(tiled, false, false, client).unwrap(),
                    2 => tab
                        .new_pane(
                            PaneId::Terminal(TerminalId::new(2)),
                            None,
                            None,
                            false,
                            true,
                            NewPanePlacement::default(),
                            Some(client),
                            None,
                        )
                        .unwrap(),
                    _ => {
                        let floating = PaneId::Terminal(TerminalId::new(2));
                        tab.new_floating_pane(floating, None, None, false, false, None, None)
                            .unwrap();
                        tab.focus_pane_with_id(floating, false, false, client)
                            .unwrap();
                        assert_eq!(tab.get_active_pane_id(client), Some(floating));
                        tab.hide_floating_panes();
                        assert_eq!(tab.get_active_pane_id(client), Some(tiled));
                    },
                }
                assert!(!tab.overlay_holds_focus(client));
                assert!(!tab.overlay_panes.is_hidden(&panel));
                assert!(!tab.overlay_panes.is_hidden(&protected));
                assert_eq!(tab.get_active_pane_id(other), Some(protected));

                tab.focus_pane_with_id(tiled, false, false, other).unwrap();
                assert!(tab.overlay_panes.is_hidden(&panel));
                assert!(tab.overlay_panes.is_hidden(&protected));
                assert_eq!(tab.get_active_pane_id(other), Some(tiled));
                tab.focus_pane_with_id(panel, true, false, other).unwrap();
                assert!(!tab.overlay_panes.is_hidden(&panel));
                assert!(!tab.overlay_panes.is_hidden(&protected));
                assert_eq!(tab.get_active_pane_id(other), Some(panel));

                assert!(tab.hide_overlay_pane(panel));
                assert!(tab.overlay_panes.is_hidden(&panel));
                assert!(tab.overlay_panes.is_hidden(&protected));
            }
        }
    }
}

#[test]
fn panel_dismissal_ignores_another_clients_unrelated_overlay_focus() {
    for companion_active in [false, true] {
        let (mut tab, _receiver) =
            create_new_tab_with_plugin_receiver(Size { cols: 80, rows: 24 }, ModeInfo::default());
        let client = ClientId::new(1);
        let other = ClientId::new(2);
        let tiled = PaneId::Terminal(TerminalId::new(1));
        let panel = add_agents_panel(&mut tab, companion_active, client);
        let unrelated = PaneId::Plugin(PluginId::new(101));
        tab.connected_clients.borrow_mut().insert(other);
        tab.new_pane(
            unrelated,
            None,
            None,
            false,
            true,
            NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
            Some(other),
            None,
        )
        .unwrap();
        assert_eq!(tab.get_active_pane_id(client), Some(panel));
        assert_eq!(tab.get_active_pane_id(other), Some(unrelated));

        tab.focus_pane_with_id(tiled, false, false, client).unwrap();

        assert!(tab.overlay_panes.is_hidden(&panel));
        assert_eq!(tab.get_active_pane_id(client), Some(tiled));
        assert!(!tab.overlay_panes.is_hidden(&unrelated));
        assert_eq!(tab.get_active_pane_id(other), Some(unrelated));
    }
}

fn add_agents_panel(tab: &mut Tab, companion_active: bool, client: ClientId) -> PaneId {
    let panel = PaneId::Plugin(PluginId::new(100));
    let run = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        if companion_active {
            "yinz:fixture-plugin-for-tests"
        } else {
            "yinz:agents"
        },
        &None,
        None,
        None,
    )
    .unwrap();
    tab.new_pane(
        panel,
        None,
        Some(Run::Plugin(run)),
        false,
        true,
        NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
        Some(client),
        None,
    )
    .unwrap();
    {
        let mut panel_layout = layout();
        panel_layout.tabs[1].location = "yinz:agents".into();
        let info = tab.mount_pane_tabs(panel, panel_layout, 0, client).unwrap();
        tab.mark_overlay_tab_group(info.id).unwrap();
    }
    panel
}

#[test]
fn panel_dismissal_preserves_modal_focus_and_unrelated_overlays() {
    for modal in [false, true] {
        let (mut tab, receiver) =
            create_new_tab_with_plugin_receiver(Size { cols: 80, rows: 24 }, ModeInfo::default());
        let client = ClientId::new(1);
        let tiled = PaneId::Terminal(TerminalId::new(1));
        let floating = PaneId::Terminal(TerminalId::new(2));
        let panel = PaneId::Plugin(PluginId::new(100));
        let other = PaneId::Plugin(PluginId::new(101));
        let run = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
            "yinz:agents",
            &None,
            None,
            None,
        )
        .unwrap();
        tab.new_pane(
            panel,
            None,
            Some(Run::Plugin(run)),
            false,
            true,
            NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
            Some(client),
            None,
        )
        .unwrap();
        let info = tab.mount_pane_tabs(panel, layout(), 0, client).unwrap();
        tab.mark_overlay_tab_group(info.id).unwrap();
        tab.new_floating_pane(floating, None, None, false, false, None, None)
            .unwrap();
        tab.focus_pane_with_id(floating, false, false, client)
            .unwrap();
        assert_eq!(tab.get_active_pane_id(client), Some(floating));
        assert!(!tab.overlay_panes.is_hidden(&panel));
        tab.new_pane(
            other,
            None,
            None,
            false,
            true,
            NewPanePlacement::Overlay(if modal {
                OverlayPlacement::modal(None)
            } else {
                OverlayPlacement::dismissible(None)
            }),
            Some(client),
            None,
        )
        .unwrap();
        receiver.try_iter().for_each(drop);
        tab.focus_pane_with_id(tiled, false, false, client).unwrap();
        assert_eq!(tab.overlay_panes.is_hidden(&panel), !modal);
        assert!(!tab.overlay_panes.is_hidden(&other));
        assert_eq!(
            tab.get_active_pane_id(client),
            Some(if modal { other } else { tiled })
        );
        let panel_hidden = receiver.try_iter().any(|(instruction, _)| {
            matches!(instruction, PluginInstruction::Update(updates) if updates.contains(&(
                Some(PluginId::new(100)), None, Event::Visible(false)
            )))
        });
        assert_eq!(panel_hidden, !modal);
        if modal {
            tab.hide_overlay_pane(other);
            tab.focus_pane_with_id(tiled, false, false, client).unwrap();
            assert!(tab.overlay_panes.is_hidden(&panel));
        }
        tab.focus_pane_with_id(panel, true, false, client).unwrap();
        assert!(!tab.overlay_panes.is_hidden(&panel));
        assert_eq!(tab.get_active_pane_id(client), Some(panel));
    }
}
