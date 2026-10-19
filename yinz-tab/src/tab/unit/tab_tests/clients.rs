use super::*;
use crate::plugins::PluginInstruction;
use yinz_bus::loading_indication::LoadingIndication;
use yinz_types::data::Event;
use yinz_types::position::Position;

#[test]
fn plugin_output_survives_late_loading_animation_ticks_and_stage_updates() {
    let mut tab = create_new_tab(
        Size {
            cols: 121,
            rows: 20,
        },
        false,
    );
    let (sender, _receiver) = unbounded();
    tab.senders
        .replace_to_plugin(SenderWithContext::new(sender));
    let client = ClientId::new(1);
    let plugin = crate::PluginId::new(2);
    let pane_id = PaneId::Plugin(plugin);
    tab.new_pane(
        pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client),
        None,
    )
    .unwrap();

    for text in ["first plugin frame", "reloaded plugin frame"] {
        tab.start_plugin_loading_indication(plugin, LoadingIndication::new("test".into()));
        assert!(!tab
            .get_pane_with_id(pane_id)
            .unwrap()
            .dump_screen(false, Some(client))
            .contains("plugin frame"));
        tab.handle_plugin_bytes(plugin, client, text.as_bytes().to_vec())
            .unwrap();
        tab.progress_plugin_loading_offset(plugin);
        tab.update_plugin_loading_stage(
            plugin,
            LoadingIndication::new("late loading stage".into()),
        );
        assert!(tab
            .get_pane_with_id(pane_id)
            .unwrap()
            .dump_screen(false, Some(client))
            .contains(text));
    }
}

#[test]
fn joining_clients_see_plugin_errors_without_plugin_bytes() {
    for (placement, suppressed) in [
        (NewPanePlacement::default(), false),
        (NewPanePlacement::Floating(None), false),
        (
            NewPanePlacement::Overlay(OverlayPlacement::default()),
            false,
        ),
        (NewPanePlacement::default(), true),
    ] {
        let mut tab = create_new_tab(
            Size {
                cols: 121,
                rows: 20,
            },
            false,
        );
        let (sender, _receiver) = unbounded();
        tab.senders
            .replace_to_plugin(SenderWithContext::new(sender));
        let first = ClientId::new(1);
        let joining = ClientId::new(2);
        let plugin = PaneId::Plugin(crate::PluginId::new(2));
        tab.new_pane(
            plugin,
            None,
            None,
            suppressed,
            true,
            placement,
            Some(first),
            None,
        )
        .unwrap();
        let pane = tab.get_pane_with_id_mut(plugin).unwrap();
        let mut loading = LoadingIndication::new("test".into());
        loading.indicate_loading_error("plugin failed to load".into());
        loading.ended = true;
        pane.update_loading_indication(loading);

        tab.add_client(joining, None).unwrap();

        let pane = tab.get_pane_with_id_mut(plugin).unwrap();
        assert!(pane
            .dump_screen(false, Some(joining))
            .contains("plugin failed to load"));
        pane.handle_plugin_bytes(joining, b"client-specific output".to_vec());
        tab.remove_client(joining); // Switching tabs retains the client's grid.
        tab.add_client(joining, None).unwrap();
        assert!(tab
            .get_pane_with_id(plugin)
            .unwrap()
            .dump_screen(false, Some(joining))
            .contains("client-specific output"));
    }
}

#[test]
fn removing_a_client_forgets_its_mode_info() {
    let mut tab = create_new_tab(
        Size {
            cols: 121,
            rows: 20,
        },
        false,
    );
    let joining = crate::ClientId::new(2);
    tab.add_client(joining, None).unwrap();
    assert!(
        tab.mode_info.borrow().contains_key(&joining),
        "precondition"
    );

    tab.remove_client(joining);

    assert!(
        !tab.mode_info.borrow().contains_key(&joining),
        "a detached client's ModeInfo used to stay in the tab for the session's life"
    );
    assert!(!tab.has_connected_client(&joining));
}

#[test]
fn a_client_detaching_does_not_forget_which_pane_the_layout_focused() {
    let layout = TiledPaneLayout {
        children: vec![
            TiledPaneLayout::default(),
            TiledPaneLayout {
                focus: Some(true),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let mut tab = create_new_tab_with_layout(
        Size {
            cols: 121,
            rows: 20,
        },
        layout,
    );
    let first = crate::ClientId::new(1);
    let focused = tab
        .get_active_pane_id(first)
        .expect("the layout focused a pane");
    assert_ne!(
        focused,
        PaneId::Terminal(TerminalId::new(0)),
        "precondition: the layout's focus pane is not the lowest id"
    );
    let second = crate::ClientId::new(2);
    tab.add_client(second, None).unwrap();

    // One of two clients detaching used to clear the layout's focus for everyone.
    tab.remove_client(second);
    tab.remove_client(first);
    let third = crate::ClientId::new(3);
    tab.add_client(third, None).unwrap();

    assert_eq!(
        tab.get_active_pane_id(third),
        Some(focused),
        "a fresh attach lands on the layout's focus pane, not on the lowest pane id"
    );
}

#[test]
fn a_fresh_client_preserves_fullscreen_when_the_layout_focus_is_hidden() {
    for covers_ui in [false, true] {
        let layout = TiledPaneLayout {
            children: vec![
                TiledPaneLayout::default(),
                TiledPaneLayout {
                    focus: Some(true),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let mut tab = create_new_tab_with_layout(
            Size {
                cols: 121,
                rows: 20,
            },
            layout,
        );
        let first = ClientId::new(1);
        let layout_focus = tab.get_active_pane_id(first).unwrap();
        let fullscreen_pane = PaneId::Terminal(TerminalId::new(0));
        assert_ne!(layout_focus, fullscreen_pane);
        tab.focus_pane_with_id(fullscreen_pane, false, false, first)
            .unwrap();
        if covers_ui {
            tab.toggle_active_pane_no_ui_fullscreen(first);
        } else {
            tab.toggle_active_pane_fullscreen(first);
        }
        assert!(tab.is_fullscreen_active());
        assert!(tab.tiled_panes.panes_to_hide_contains(layout_focus));
        tab.remove_client(first);
        assert!(tab.has_no_connected_clients());
        let joining = ClientId::new(2);

        tab.add_client(joining, None).unwrap();

        assert!(tab.has_connected_client(&joining));
        assert!(tab.is_fullscreen_active(), "covers_ui={covers_ui}");
        assert_eq!(tab.fullscreen_covers_ui(), covers_ui);
        assert_eq!(tab.get_active_pane_id(joining), Some(fullscreen_pane));
        assert_eq!(tab.tiled_panes.fullscreen_pane_id(), Some(fullscreen_pane));
        assert!(tab.tiled_panes.panes_to_hide_contains(layout_focus));
        assert!(!tab.tiled_panes.panes_to_hide_contains(fullscreen_pane));
    }
}

#[test]
fn a_closed_focus_pane_is_not_offered_to_a_joining_client() {
    let layout = TiledPaneLayout {
        children: vec![
            TiledPaneLayout::default(),
            TiledPaneLayout {
                focus: Some(true),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let mut tab = create_new_tab_with_layout(
        Size {
            cols: 121,
            rows: 20,
        },
        layout,
    );
    let first = crate::ClientId::new(1);
    let focused = tab
        .get_active_pane_id(first)
        .expect("the layout focused a pane");
    tab.close_pane(focused, false, None);
    tab.remove_client(first);
    let joining = crate::ClientId::new(2);

    tab.add_client(joining, None).unwrap();

    let landed = tab.get_active_pane_id(joining);
    assert!(landed.is_some() && landed != Some(focused), "{landed:?}");
}

#[test]
fn removing_clients_preserves_shared_hover_until_the_last_client_leaves() {
    use yinz_types::data::{HighlightLayer, HighlightStyle, RegexHighlight};
    for first in [ClientId::new(1), ClientId::new(2)] {
        let mut tab = create_new_tab(Size { cols: 80, rows: 24 }, false);
        let clients = [ClientId::new(1), ClientId::new(2)];
        let pane_id = tab.get_active_pane_id(clients[0]).unwrap();
        for client in clients {
            tab.mouse_hover_pane_id.insert(client, pane_id);
        }
        let pane = tab.get_pane_with_id_mut(pane_id).unwrap();
        pane.set_plugin_regex_highlights(
            crate::PluginId::new(1),
            vec![RegexHighlight {
                pattern: "test".into(),
                style: HighlightStyle::None,
                layer: HighlightLayer::Hint,
                context: Default::default(),
                on_hover: true,
                bold: false,
                italic: false,
                underline: false,
                tooltip_text: None,
            }],
            &Default::default(),
        );
        let position = Some(Position::new(0, 0));
        assert!(pane.set_hover_position(position));
        tab.remove_client_pane_state(first);
        assert!(!tab
            .get_pane_with_id_mut(pane_id)
            .unwrap()
            .set_hover_position(position));
        let last = clients.into_iter().find(|id| *id != first).unwrap();
        assert_eq!(tab.mouse_hover_pane_id.get(&last), Some(&pane_id));
        tab.remove_client_pane_state(last);
        assert!(!tab
            .get_pane_with_id_mut(pane_id)
            .unwrap()
            .set_hover_position(None));
        assert!(tab.mouse_hover_pane_id.is_empty());
    }
}

#[test]
fn disconnect_only_forces_render_for_members_or_stale_dimming() {
    for (client, dimmed, redraw) in [(99, false, false), (1, false, true), (99, true, true)] {
        let mut tab = create_new_tab(
            Size {
                cols: 121,
                rows: 20,
            },
            false,
        );
        let client = ClientId::new(client);
        if dimmed {
            tab.set_client_dimmed(client, true);
        }
        assert!(!tab.tiled_panes.panes.is_empty());
        for pane in tab.tiled_panes.panes.values_mut() {
            pane.set_should_render(false);
        }
        tab.remove_client(client);
        assert!(!tab.dimmed_clients.contains(&client));
        assert!(tab
            .tiled_panes
            .panes
            .values()
            .all(|pane| pane.should_render() == redraw));
    }
}

#[test]
fn suppressed_plugins_receive_tab_mode_updates_once() {
    let mut tab = create_new_tab(
        Size {
            cols: 121,
            rows: 20,
        },
        false,
    );
    let (sender, receiver) = unbounded();
    tab.senders
        .replace_to_plugin(SenderWithContext::new(sender));
    let client = ClientId::new(1);
    let plugin = crate::PluginId::new(9);
    tab.new_pane(
        PaneId::Plugin(plugin),
        None,
        None,
        true,
        true,
        NewPanePlacement::default(),
        Some(client),
        None,
    )
    .unwrap();
    assert!(!tab.get_plugin_ids().contains(&plugin));
    assert_eq!(tab.get_plugin_ids_including_suppressed(), vec![plugin]);
    receiver.try_iter().for_each(drop);
    tab.update_input_modes().unwrap();
    let updates: Vec<_> = receiver
        .try_iter()
        .flat_map(|(instruction, _)| match instruction {
            PluginInstruction::Update(updates) => updates,
            _ => vec![],
        })
        .collect();
    assert_eq!(updates.len(), 1);
    assert!(
        matches!(&updates[0], (Some(pid), Some(cid), Event::ModeUpdate(_)) if *pid == plugin && *cid == client)
    );
}

#[test]
fn removing_a_clients_pane_state_forgets_where_it_was_focused() {
    let mut tab = create_new_tab(Size { cols: 80, rows: 24 }, false);
    let (departed, staying) = (ClientId::new(1), ClientId::new(2));
    tab.add_client(staying, None).unwrap();
    let pane_id = tab.get_active_pane_id(departed).unwrap();
    assert_eq!(tab.get_active_pane_id(staying), Some(pane_id));
    tab.remove_client(departed);
    tab.remove_client_pane_state(departed);
    assert!(!tab.tiled_panes.active_panes().contains_key(&departed));
    assert!(tab.tiled_panes.active_panes().contains_key(&staying));
}

#[test]
fn a_client_joining_an_empty_tab_lands_where_the_last_one_left() {
    let mut tab = create_new_tab(
        Size {
            cols: 121,
            rows: 20,
        },
        false,
    );
    let (departed, joining) = (ClientId::new(1), ClientId::new(2));
    let second = PaneId::Terminal(crate::TerminalId::new(2));
    tab.new_pane(
        second,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(departed),
        None,
    )
    .unwrap();
    tab.focus_pane_with_id(second, false, false, departed)
        .unwrap();
    assert_eq!(tab.get_active_pane_id(departed), Some(second));
    tab.remember_departing_focus(departed);
    tab.remove_client_pane_state(departed);
    tab.remove_client(departed);

    tab.add_client(joining, None).unwrap();

    assert_eq!(tab.get_active_pane_id(joining), Some(second));
}

/// The screen offers every tab a disconnecting client's focus. A tab it was not in is already
/// empty, and must keep the place its own last client left.
#[test]
fn another_clients_departure_keeps_an_empty_tabs_remembered_focus() {
    let mut tab = create_new_tab(
        Size {
            cols: 121,
            rows: 20,
        },
        false,
    );
    let (departed, elsewhere, joining) = (ClientId::new(1), ClientId::new(2), ClientId::new(3));
    let second = PaneId::Terminal(crate::TerminalId::new(2));
    tab.new_pane(
        second,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(departed),
        None,
    )
    .unwrap();
    tab.focus_pane_with_id(second, false, false, departed)
        .unwrap();
    tab.remember_departing_focus(departed);
    tab.remove_client_pane_state(departed);
    tab.remove_client(departed);

    tab.remember_departing_focus(elsewhere);
    tab.add_client(joining, None).unwrap();

    assert_eq!(tab.get_active_pane_id(joining), Some(second));
}

/// A client coming back from another tab resumes its own floating focus, not the one the tab's
/// last client left behind.
#[test]
fn a_returning_client_keeps_its_own_floating_focus() {
    let mut tab = create_tab_with_two_floating_panes();
    let (departed, returning) = (ClientId::new(1), ClientId::new(2));
    let (remembered, own) = (
        PaneId::Terminal(TerminalId::new(2)),
        PaneId::Terminal(TerminalId::new(3)),
    );
    tab.floating_panes.focus_pane(remembered, departed);
    tab.remember_departing_focus(departed);
    tab.remove_client_pane_state(departed);
    tab.remove_client(departed);
    tab.add_client(returning, None).unwrap();
    assert_eq!(
        tab.floating_panes.active_pane_id(returning),
        Some(remembered)
    );
    tab.floating_panes.focus_pane(own, returning);

    tab.remove_client(returning);
    tab.add_client(returning, None).unwrap();

    assert_eq!(tab.floating_panes.active_pane_id(returning), Some(own));
}

/// A client that dismissed the overlay the tab's last client left focused is not put back on
/// it each time it returns from another tab.
#[test]
fn a_returning_client_is_not_refocused_on_an_overlay_it_dismissed() {
    let mut tab = create_new_tab(
        Size {
            cols: 121,
            rows: 20,
        },
        false,
    );
    let (departed, returning) = (ClientId::new(1), ClientId::new(2));
    let overlay = PaneId::Terminal(TerminalId::new(2));
    tab.new_pane(
        overlay,
        None,
        None,
        false,
        true,
        NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
        Some(departed),
        None,
    )
    .unwrap();
    assert_eq!(tab.overlay_panes.active_pane_id(departed), Some(overlay));
    tab.remember_departing_focus(departed);
    tab.remove_client_pane_state(departed);
    tab.remove_client(departed);
    tab.add_client(returning, None).unwrap();
    assert_eq!(tab.overlay_panes.active_pane_id(returning), Some(overlay));
    assert!(tab.overlay_panes.defocus_pane_if_dismissible(returning));

    tab.remove_client(returning);
    tab.add_client(returning, None).unwrap();

    assert_eq!(tab.overlay_panes.active_pane_id(returning), None);
}
