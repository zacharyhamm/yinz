use super::*;

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
