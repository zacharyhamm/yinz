use super::*;

#[test]
fn fit_disabled_excludes_web_client_from_min_size() {
    let initial_size = Size { cols: 80, rows: 20 };
    let mut screen = create_new_screen(initial_size, true, true);
    new_tab(&mut screen, 1, 0);
    screen
        .add_client(2, /* is_web_client */ true)
        .expect("TEST");
    screen.set_client_size(1, Size { cols: 80, rows: 20 });
    screen.set_client_size(2, Size { cols: 40, rows: 10 });

    screen
        .set_mobile_render_preferences(2, /* single_pane */ false, /* fit */ false)
        .expect("TEST");

    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size { cols: 80, rows: 20 },
        "Fit-disabled web client is excluded from the min-size loop; tab stays at desktop size"
    );
}

#[test]
fn fit_disabled_ignores_client_on_another_tab() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_non_mirrored_screen(initial_size);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen
        .add_client(2, /* is_web_client */ false)
        .expect("TEST");
    screen.set_client_size(1, Size { cols: 40, rows: 10 });
    screen.set_client_size(
        2,
        Size {
            cols: 160,
            rows: 50,
        },
    );
    screen.switch_active_tab(0, None, true, 1).expect("TEST");
    screen.switch_active_tab(1, None, true, 2).expect("TEST");

    screen
        .set_mobile_render_preferences(1, /* single_pane */ false, /* fit */ false)
        .expect("TEST");

    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size { cols: 40, rows: 10 },
        "A client on another tab is not a reference: fit is forced on and the mobile size drives layout"
    );
}

#[test]
fn fit_disabled_tab_repins_on_desktop_resize() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_non_mirrored_screen(initial_size);
    new_tab(&mut screen, 1, 0);
    screen
        .add_client(2, /* is_web_client */ false)
        .expect("TEST");
    screen.set_client_size(1, Size { cols: 40, rows: 10 });
    screen.set_client_size(
        2,
        Size {
            cols: 160,
            rows: 50,
        },
    );
    screen.switch_active_tab(0, None, true, 1).expect("TEST");
    screen.switch_active_tab(0, None, true, 2).expect("TEST");

    screen
        .set_mobile_render_preferences(1, /* single_pane */ false, /* fit */ false)
        .expect("TEST");
    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size {
            cols: 160,
            rows: 50,
        },
        "Pre-condition: shared tab pinned to the initial desktop reference size"
    );

    // Desktop client resizes; the fit-disabled tab must re-pin to its new size.
    screen.set_client_size(
        2,
        Size {
            cols: 120,
            rows: 40,
        },
    );
    screen.resize_to_clients().expect("TEST");

    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size {
            cols: 120,
            rows: 40,
        },
        "Tab re-pins to the new desktop reference size after a desktop resize"
    );
}

#[test]
fn fit_disabled_reverts_when_reference_client_switches_tab() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_non_mirrored_screen(initial_size);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen
        .add_client(2, /* is_web_client */ false)
        .expect("TEST");
    screen.set_client_size(1, Size { cols: 40, rows: 10 });
    screen.set_client_size(
        2,
        Size {
            cols: 160,
            rows: 50,
        },
    );
    screen.switch_active_tab(0, None, true, 1).expect("TEST");
    screen.switch_active_tab(0, None, true, 2).expect("TEST");

    screen
        .set_mobile_render_preferences(1, /* single_pane */ false, /* fit */ false)
        .expect("TEST");
    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size {
            cols: 160,
            rows: 50,
        },
        "Pre-condition: tab is shared, so fit-disabled is honored"
    );

    // The reference client leaves the tab without disconnecting.
    screen.switch_active_tab(1, None, true, 2).expect("TEST");
    screen.log_and_report_session_state().expect("TEST");

    assert!(
        screen.mobile_web_prefs.get(&1).map(|prefs| prefs.fit) == Some(true),
        "Fit reverts once no client shares the tab, without any disconnect"
    );
    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size { cols: 40, rows: 10 },
        "The tab shrinks to the mobile size once nobody else is viewing it"
    );
}

#[test]
fn fit_disabled_allowed_when_only_other_client_is_web() {
    let initial_size = Size { cols: 80, rows: 20 };
    let mut screen = create_new_screen(initial_size, true, true);
    new_tab(&mut screen, 1, 0);
    screen
        .add_client(1, /* is_web_client */ true)
        .expect("TEST");
    screen
        .add_client(2, /* is_web_client */ true)
        .expect("TEST");
    screen.set_client_size(1, Size { cols: 80, rows: 20 });
    screen.set_client_size(2, Size { cols: 40, rows: 10 });

    screen
        .set_mobile_render_preferences(2, /* single_pane */ false, /* fit */ false)
        .expect("TEST");

    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size { cols: 80, rows: 20 },
        "A web client counts as a reference; fit-disabled is honored and the tab stays at the reference size"
    );
}

#[test]
fn fit_disabled_without_desktop_client_forces_enabled() {
    let initial_size = Size { cols: 80, rows: 20 };
    let mut screen = create_new_screen(initial_size, true, true);
    new_tab(&mut screen, 1, 0);
    screen
        .add_client(1, /* is_web_client */ true)
        .expect("TEST");
    screen.set_client_size(1, Size { cols: 40, rows: 10 });

    screen
        .set_mobile_render_preferences(1, /* single_pane */ false, /* fit */ false)
        .expect("TEST");

    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size { cols: 40, rows: 10 },
        "With no desktop client, fit is forced enabled and the mobile size drives layout"
    );
}

#[test]
fn desktop_disconnect_reverts_fit_disabled() {
    let initial_size = Size { cols: 80, rows: 20 };
    let mut screen = create_new_screen(initial_size, true, true);
    new_tab(&mut screen, 1, 0);
    screen
        .add_client(2, /* is_web_client */ true)
        .expect("TEST");
    screen.set_client_size(1, Size { cols: 80, rows: 20 });
    screen.set_client_size(2, Size { cols: 40, rows: 10 });

    screen
        .set_mobile_render_preferences(2, /* single_pane */ false, /* fit */ false)
        .expect("TEST");
    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size { cols: 80, rows: 20 },
        "Pre-condition: fit-disabled excludes the mobile client"
    );

    screen.remove_client(1).expect("TEST");

    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size { cols: 40, rows: 10 },
        "Desktop disconnect reverts the mobile client to fit-enabled; its size now drives layout"
    );
}

#[test]
fn single_pane_fullscreens_active_pane_and_reverts() {
    let initial_size = Size { cols: 80, rows: 20 };
    let mut screen = create_new_screen(initial_size, true, true);
    new_tab(&mut screen, 1, 0);
    add_second_pane_to_active_tab(&mut screen, 2);
    let client = 1;

    screen
        .set_mobile_render_preferences(client, /* single_pane */ true, /* fit */ true)
        .expect("TEST");
    assert!(
        screen.tabs.get(&0).unwrap().is_fullscreen_active(),
        "Single-pane fullscreens the active pane"
    );
    assert!(
        screen.tabs.get(&0).unwrap().fullscreen_covers_ui(),
        "Single-pane hides the tab bar and status bar"
    );

    screen
        .set_mobile_render_preferences(client, /* single_pane */ false, /* fit */ true)
        .expect("TEST");
    assert!(
        !screen.tabs.get(&0).unwrap().is_fullscreen_active(),
        "Disabling single-pane reverts the fullscreen"
    );
}

#[test]
fn focus_pane_by_id_is_per_client_and_does_not_steal_global_focus() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_non_mirrored_screen(initial_size);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen
        .add_client(2, /* is_web_client */ false)
        .expect("TEST");
    screen.switch_active_tab(0, None, true, 1).expect("TEST");
    screen.switch_active_tab(1, None, true, 2).expect("TEST");

    assert_eq!(
        screen.get_active_pane_id(&2),
        Some(PaneId::Terminal(2)),
        "Pre-condition: client 2 is focused on pane 2 (its own tab)"
    );

    // Client 1 focuses the pane living on client 2's tab.
    screen
        .focus_pane_with_id(PaneId::Terminal(2), false, false, 1)
        .expect("TEST");

    assert_eq!(
        screen.get_active_pane_id(&1),
        Some(PaneId::Terminal(2)),
        "Client 1 now focuses the target pane"
    );
    assert_eq!(
        screen.active_tab_ids.get(&2).copied(),
        Some(1),
        "Client 2's active tab is unchanged by client 1's per-client focus"
    );
    assert_eq!(
        screen.get_active_pane_id(&2),
        Some(PaneId::Terminal(2)),
        "Client 2's focus is not stolen by client 1's per-client focus"
    );
}

#[test]
fn mobile_state_reports_single_pane_off_before_any_preference_is_sent() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, messages) = create_new_screen_with_message_capture(size);
    new_tab(&mut screen, 1, 0);
    screen
        .add_client(2, /* is_web_client */ true)
        .expect("TEST");

    screen.log_and_report_session_state().expect("TEST");

    let msgs = messages.lock().unwrap();
    let payload = msgs
        .get(&2)
        .expect("TEST")
        .iter()
        .find_map(|msg| match msg {
            ServerToClientMsg::MobileState { payload } => Some(payload.clone()),
            _ => None,
        })
        .expect("A web client receives a MobileState push");

    assert!(
        !payload.render_prefs.single_pane,
        "A client that never sent preferences has nothing fullscreened, so single-pane must be reported off"
    );
    assert!(
        payload.render_prefs.fit,
        "Such a client participates in the tab min-size rule, so fit is reported on"
    );
    assert!(
        !screen.tabs.get(&0).unwrap().is_fullscreen_active(),
        "Nothing is fullscreened for a client that never sent preferences"
    );
}

#[test]
fn single_pane_demotes_after_desktop_unfullscreen() {
    let initial_size = Size { cols: 80, rows: 20 };
    let mut screen = create_new_screen(initial_size, true, true);
    new_tab(&mut screen, 1, 0);
    add_second_pane_to_active_tab(&mut screen, 2);
    let client = 1;

    screen
        .set_mobile_render_preferences(client, /* single_pane */ true, /* fit */ true)
        .expect("TEST");
    assert!(
        screen.tabs.get(&0).unwrap().is_fullscreen_active(),
        "Pre-condition: single-pane fullscreen active"
    );

    // Simulate a desktop-initiated un-fullscreen on the same tab.
    screen.tabs.get_mut(&0).unwrap().unset_fullscreen();
    assert!(
        !screen.tabs.get(&0).unwrap().is_fullscreen_active(),
        "Fullscreen cleared by the simulated desktop action"
    );

    screen.log_and_report_session_state().expect("TEST");

    assert!(
        !screen.tabs.get(&0).unwrap().is_fullscreen_active(),
        "Single-pane yields to the un-fullscreen instead of re-asserting it"
    );
    assert_eq!(
        screen
            .mobile_web_prefs
            .get(&client)
            .map(|prefs| prefs.single_pane),
        Some(false),
        "The client is demoted out of single-pane mode and reports it"
    );
    assert_eq!(
        screen
            .mobile_web_prefs
            .get(&client)
            .and_then(|prefs| prefs.fullscreened_pane),
        None,
        "The tracked fullscreen pane is cleared on demotion"
    );
}

#[test]
fn single_pane_demotion_disables_fit_when_tab_is_shared() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_non_mirrored_screen(initial_size);
    new_tab(&mut screen, 1, 0);
    add_second_pane_to_active_tab(&mut screen, 2);
    screen
        .add_client(2, /* is_web_client */ false)
        .expect("TEST");
    screen.set_client_size(1, Size { cols: 40, rows: 10 });
    screen.set_client_size(
        2,
        Size {
            cols: 160,
            rows: 50,
        },
    );
    screen.switch_active_tab(0, None, true, 1).expect("TEST");
    screen.switch_active_tab(0, None, true, 2).expect("TEST");

    screen
        .set_mobile_render_preferences(1, /* single_pane */ true, /* fit */ true)
        .expect("TEST");

    // Simulate a desktop-initiated un-fullscreen on the shared tab.
    screen.tabs.get_mut(&0).unwrap().unset_fullscreen();
    screen.log_and_report_session_state().expect("TEST");

    assert_eq!(
        screen.mobile_web_prefs.get(&1).map(|prefs| prefs.fit),
        Some(false),
        "Demotion also disables fit so the shared tab is not shrunk to the mobile size"
    );
    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size {
            cols: 160,
            rows: 50,
        },
        "The shared tab keeps the desktop size after the demotion"
    );
}

#[test]
fn single_pane_demotes_when_desktop_opens_a_pane() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_non_mirrored_screen(initial_size);
    new_tab(&mut screen, 1, 0);
    add_second_pane_to_active_tab(&mut screen, 2);
    let mobile_client = 1;
    screen
        .add_client(2, /* is_web_client */ false)
        .expect("TEST");
    screen.switch_active_tab(0, None, true, 1).expect("TEST");
    screen.switch_active_tab(0, None, true, 2).expect("TEST");

    screen
        .set_mobile_render_preferences(
            mobile_client,
            /* single_pane */ true,
            /* fit */ true,
        )
        .expect("TEST");
    assert!(
        screen.tabs.get(&0).unwrap().is_fullscreen_active(),
        "Pre-condition: single-pane fullscreen active"
    );

    // The desktop client opens a pane in the shared tab, which drops fullscreen.
    screen
        .get_active_tab_mut(2)
        .unwrap()
        .new_pane(
            PaneId::Terminal(3),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(2),
            None,
        )
        .expect("TEST");
    screen.log_and_report_session_state().expect("TEST");

    assert!(
        !screen.tabs.get(&0).unwrap().is_fullscreen_active(),
        "The pane the desktop client opened stays visible"
    );
    assert_eq!(
        screen
            .mobile_web_prefs
            .get(&mobile_client)
            .map(|prefs| prefs.single_pane),
        Some(false),
        "The mobile client is demoted rather than hiding the new pane"
    );
}

#[test]
fn single_pane_follows_focus_to_newly_added_pane() {
    let initial_size = Size { cols: 80, rows: 20 };
    let mut screen = create_new_screen(initial_size, true, true);
    new_tab(&mut screen, 1, 0);
    add_second_pane_to_active_tab(&mut screen, 2);
    let client = 1;

    screen
        .set_mobile_render_preferences(client, /* single_pane */ true, /* fit */ true)
        .expect("TEST");
    assert_eq!(
        screen.tabs.get(&0).unwrap().fullscreen_pane_id(),
        Some(PaneId::Terminal(2)),
        "Pre-condition: single-pane fullscreens the current active pane"
    );

    add_second_pane_to_active_tab(&mut screen, 3);
    screen.log_and_report_session_state().expect("TEST");

    assert!(
        screen.tabs.get(&0).unwrap().is_fullscreen_active(),
        "A pane added in single-pane mode keeps a pane fullscreened"
    );
    assert_eq!(
        screen.tabs.get(&0).unwrap().fullscreen_pane_id(),
        Some(PaneId::Terminal(3)),
        "The fullscreen follows focus to the newly added pane"
    );
    assert!(
        screen.tabs.get(&0).unwrap().fullscreen_covers_ui(),
        "The followed fullscreen still hides the tab bar and status bar"
    );
}

#[test]
fn single_pane_demotes_after_desktop_downgrades_to_regular_fullscreen() {
    let initial_size = Size { cols: 80, rows: 20 };
    let mut screen = create_new_screen(initial_size, true, true);
    new_tab(&mut screen, 1, 0);
    add_second_pane_to_active_tab(&mut screen, 2);
    let client = 1;

    screen
        .set_mobile_render_preferences(client, /* single_pane */ true, /* fit */ true)
        .expect("TEST");
    let fullscreen_pane_id = screen.tabs.get(&0).unwrap().fullscreen_pane_id().unwrap();

    // A desktop client toggles regular fullscreen, downgrading the no-ui fullscreen.
    screen
        .tabs
        .get_mut(&0)
        .unwrap()
        .toggle_pane_fullscreen(fullscreen_pane_id);
    screen.log_and_report_session_state().expect("TEST");

    assert!(
        screen.tabs.get(&0).unwrap().is_fullscreen_active(),
        "The regular fullscreen the desktop client asked for is kept"
    );
    assert!(
        !screen.tabs.get(&0).unwrap().fullscreen_covers_ui(),
        "Single-pane does not re-assert the no-ui fullscreen"
    );
    assert_eq!(
        screen
            .mobile_web_prefs
            .get(&client)
            .map(|prefs| prefs.single_pane),
        Some(false),
        "The client is demoted out of single-pane mode"
    );
}

#[test]
fn single_pane_survives_the_mobile_client_opening_its_own_pane_in_a_shared_tab() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_non_mirrored_screen(initial_size);
    new_tab(&mut screen, 1, 0);
    add_second_pane_to_active_tab(&mut screen, 2);
    let mobile_client = 1;
    screen
        .add_client(2, /* is_web_client */ false)
        .expect("TEST");
    screen.switch_active_tab(0, None, true, 1).expect("TEST");
    screen.switch_active_tab(0, None, true, 2).expect("TEST");

    screen
        .set_mobile_render_preferences(
            mobile_client,
            /* single_pane */ true,
            /* fit */ true,
        )
        .expect("TEST");
    assert!(
        screen.tabs.get(&0).unwrap().is_fullscreen_active(),
        "Pre-condition: single-pane fullscreen active"
    );

    // The mobile client opens a pane of its own, which takes its focus.
    screen
        .get_active_tab_mut(mobile_client)
        .unwrap()
        .new_pane(
            PaneId::Terminal(3),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(mobile_client),
            None,
        )
        .expect("TEST");
    screen.log_and_report_session_state().expect("TEST");

    assert_eq!(
        screen
            .mobile_web_prefs
            .get(&mobile_client)
            .map(|prefs| prefs.single_pane),
        Some(true),
        "A pane the mobile client opened itself does not demote it out of single-pane mode"
    );
    assert_eq!(
        screen.tabs.get(&0).unwrap().fullscreen_pane_id(),
        Some(PaneId::Terminal(3)),
        "The fullscreen follows the mobile client to the pane it just opened"
    );
    assert!(
        screen.tabs.get(&0).unwrap().fullscreen_covers_ui(),
        "The followed fullscreen still hides the tab bar and status bar"
    );
    assert_eq!(
        screen
            .mobile_web_prefs
            .get(&mobile_client)
            .map(|prefs| prefs.fit),
        Some(true),
        "Fit is untouched because there was no demotion"
    );
}

#[test]
fn single_pane_follows_the_mobile_client_into_a_new_tab() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_non_mirrored_screen(initial_size);
    new_tab(&mut screen, 1, 0);
    let mobile_client = 1;
    screen
        .add_client(2, /* is_web_client */ false)
        .expect("TEST");
    screen.switch_active_tab(0, None, true, 1).expect("TEST");
    screen.switch_active_tab(0, None, true, 2).expect("TEST");

    screen
        .set_mobile_render_preferences(
            mobile_client,
            /* single_pane */ true,
            /* fit */ true,
        )
        .expect("TEST");
    assert!(
        screen.tabs.get(&0).unwrap().is_fullscreen_active(),
        "Pre-condition: single-pane fullscreen active"
    );

    new_tab(&mut screen, 2, 1);

    assert_eq!(
        screen
            .mobile_web_prefs
            .get(&mobile_client)
            .map(|prefs| prefs.single_pane),
        Some(true),
        "Opening a tab keeps the mobile client in single-pane mode"
    );
    assert!(
        !screen.tabs.get(&0).unwrap().is_fullscreen_active(),
        "The tab the mobile client left is restored for the desktop client"
    );
    assert_eq!(
        screen.tabs.get(&1).unwrap().fullscreen_pane_id(),
        Some(PaneId::Terminal(2)),
        "The pane of the new tab is fullscreened for the mobile client"
    );
    assert!(
        screen.tabs.get(&1).unwrap().fullscreen_covers_ui(),
        "The new tab's fullscreen still hides the tab bar and status bar"
    );
}
