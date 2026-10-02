use super::*;
use yinz_bus::loading_indication::LoadingIndication;
use yinz_types::data::{PermissionType, PluginPermission};

#[test]
fn joining_clients_see_plugin_errors_and_permission_prompts_without_plugin_bytes() {
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
        pane.request_permissions_from_user(Some(PluginPermission::new(
            "test".into(),
            vec![PermissionType::ReadApplicationState],
        )));
        tab.remove_client_pane_state(joining);
        tab.remove_client(joining);
        tab.add_client(joining, None).unwrap();
        let pane = tab.get_pane_with_id_mut(plugin).unwrap();
        let contents = pane.dump_screen(false, Some(joining));
        assert!(contents.contains("Allow?"), "{contents:?}");

        pane.request_permissions_from_user(None);
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
