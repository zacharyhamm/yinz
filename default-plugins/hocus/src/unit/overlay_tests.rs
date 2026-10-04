//! Hocus sends one placement and leaves child geometry and visibility to the host.
use super::*;

fn info() -> MultipaneInfo {
    MultipaneInfo {
        id: MultipaneId(42),
        children: [
            PaneId::Plugin(PluginId::new(9)),
            PaneId::Plugin(PluginId::new(7)),
        ],
        x: 3,
        y: 14,
        columns: 120,
        rows: 33,
        split: MultipaneSplit {
            direction: MultipaneDirection::LeftRight,
            ratio: 3700,
        },
    }
}
fn pane(id: u32, is_suppressed: bool) -> PaneInfo {
    PaneInfo {
        id: PaneId::Plugin(PluginId::new(id)),
        is_floating: true,
        is_suppressed,
        multipane: Some(info()),
        ..Default::default()
    }
}
fn manifest(tab: usize, panes: Vec<PaneInfo>) -> PaneManifest {
    PaneManifest {
        panes: std::collections::HashMap::from([(TabPosition::new(tab), panes)]),
    }
}
fn placed_state() -> State {
    State {
        own_plugin_id: PluginId::new(7),
        own_is_floating: Some(true),
        own_tab_index: Some(TabPosition::new(0)),
        agents_plugin: Some(PluginId::new(9)),
        agents_tab_index: Some(TabPosition::new(0)),
        surface_rows: 50,
        surface_cols: 200,
        multipane: Some(info()),
        placed: true,
        ..Default::default()
    }
}
fn config(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}
fn rect(coordinates: &FloatingPaneCoordinates) -> (usize, usize, usize, usize) {
    let fixed = |value| match value {
        Some(PercentOrFixed::Fixed(n)) => n,
        other => panic!("expected fixed coordinate, got {other:?}"),
    };
    (
        fixed(coordinates.x),
        fixed(coordinates.y),
        fixed(coordinates.width),
        fixed(coordinates.height),
    )
}

#[test]
fn startup_composes_existing_or_launched_agents_and_preserves_focus() {
    let mut state = placed_state();
    state.multipane = None;
    state.kdl = config(&[("split", "25%")]);
    for agents_first in [true, false] {
        state.agents_plugin = agents_first.then_some(PluginId::new(9));
        let PluginCommand::CreateMultipane(request) = state
            .placement_command(state.panel_rect().unwrap(), false)
            .unwrap()
        else {
            panic!("create")
        };
        assert_eq!(
            request.children[1],
            MultipaneChild::Existing(PaneId::Plugin(PluginId::new(7)))
        );
        if agents_first {
            assert_eq!(
                request.children[0],
                MultipaneChild::Existing(PaneId::Plugin(PluginId::new(9)))
            );
        } else {
            assert_eq!(
                request.children[0],
                MultipaneChild::Plugin {
                    url: launch::HOCUS_AGENTS_ALIAS.into(),
                    configuration: launch::overlay_configuration()
                }
            );
        }
        assert_eq!(request.focus, None);
        assert_eq!(request.split.direction, MultipaneDirection::LeftRight);
        assert!((2400..=2500).contains(&request.split.ratio));
        assert_eq!(
            rect(request.placement.coordinates.as_ref().unwrap()),
            (3, 14, 120, 33)
        );
    }
}

#[test]
fn a_reloaded_child_uses_host_metadata_and_never_resets_the_divider() {
    let mut state = State {
        own_plugin_id: PluginId::new(7),
        surface_rows: 50,
        surface_cols: 200,
        ..Default::default()
    };
    state.on_panes(manifest(2, vec![pane(7, false), pane(9, false)]));
    assert_eq!(state.agents_plugin, Some(PluginId::new(9)));
    assert_eq!(state.agents_tab_index, Some(TabPosition::new(2)));
    assert_eq!(state.multipane, Some(info()));
    for (rows, cols) in [(2, 3), (50, 200), (80, 300)] {
        state.surface_rows = rows;
        state.surface_cols = cols;
        let PluginCommand::UpdateMultipane(request) = state
            .placement_command(state.panel_rect().unwrap(), false)
            .unwrap()
        else {
            panic!("update")
        };
        assert_eq!(request.id, info().id);
        assert_eq!(request.split, None);
        assert!(!request.override_user_placement);
    }
}

#[test]
fn geometry_is_one_rectangle_clamped_on_every_surface() {
    for rows in 1..70 {
        for cols in 1..220 {
            for corner in CORNERS {
                let panel = rect_in_corner(
                    &config(&[("pad", "20"), ("width", "500%")]),
                    rows,
                    cols,
                    Shown::BOTH,
                    corner,
                    None,
                )
                .unwrap();
                assert!(panel.cols > 0 && panel.rows > 0);
                assert!(panel.x + panel.cols <= cols && panel.y + panel.rows <= rows);
            }
        }
    }
    assert!(rect_in_corner(&BTreeMap::new(), 0, 80, Shown::BOTH, Corner::TopLeft, None).is_none());
    assert_eq!(
        configured_size(&config(&[("height", "10"), ("width", "40%")]), 50, 200),
        (80, 10)
    );
    assert_eq!(
        configured_size(&config(&[("height", "bad"), ("width", "bad")]), 50, 200),
        (120, 33)
    );
}

#[test]
fn corner_cycle_keeps_user_size_and_places_only_the_container() {
    let mut state = placed_state();
    state.panel.placed_corner = Some(Corner::BottomLeft);
    for expected in [
        Corner::TopLeft,
        Corner::TopRight,
        Corner::BottomRight,
        Corner::BottomLeft,
    ] {
        state.reposition();
        assert_eq!(state.panel.placed_corner, Some(expected));
        let (placements, reclaim) = state.last_placement.as_ref().unwrap();
        assert!(*reclaim);
        assert_eq!(placements.len(), 1);
        let rectangle = rect(&placements[0].1);
        assert_eq!((rectangle.2, rectangle.3), (120, 33));
        let info = state.multipane.as_mut().unwrap();
        info.x = rectangle.0;
        info.y = rectangle.1;
        state.panel.placed_rect = state.reported_panel();
    }
}

#[test]
fn hide_reveal_and_manifest_updates_send_no_sibling_repairs() {
    let mut state = placed_state();
    for hidden in [true, false, true, false] {
        if hidden {
            state.hide_panel();
        } else {
            state.reveal_panel();
        }
        assert_eq!(
            state.visibility_calls.last(),
            Some(&(PaneId::Plugin(PluginId::new(7)), !hidden))
        );
        state.visibility_calls.clear();
        state.on_panes(manifest(0, vec![pane(7, hidden), pane(9, hidden)]));
        assert_eq!(state.hidden, hidden);
        assert!(state.visibility_calls.is_empty());
        assert_eq!(state.multipane.as_ref().unwrap().split.ratio, 3700);
    }
}

#[test]
fn tiled_hocus_does_not_create_or_hide_a_multipane() {
    let mut state = placed_state();
    state.own_is_floating = Some(false);
    state.multipane = None;
    assert!(state.placement_command(info_rect(), false).is_none());
    state.apply_overlay_geometry();
    state.hide_panel();
    assert!(state.last_placement.is_none() && state.visibility_calls.is_empty());
}
fn info_rect() -> Rect {
    Rect {
        x: 3,
        y: 14,
        cols: 120,
        rows: 33,
    }
}

fn yielded(stop: &str) -> PipeMessage {
    PipeMessage {
        source: PipeSource::Plugin(yinz_tile::prelude::PluginId::new(7)),
        name: hocus_proto::wire::AGENT_STRIP_YIELD.to_owned(),
        payload: None,
        args: BTreeMap::from([(hocus_proto::wire::PANEL_STOP.to_owned(), stop.to_owned())]),
        is_private: true,
        pane_id: None,
    }
}

// -- the ring's two crossings ------------------------------------------------

/// Stepping out of the agent list lands on the stop the list names, on the chat that is up.
#[test]
fn a_yield_lands_the_keys_on_the_stop_it_names() {
    let mut state = placed_state();

    assert!(state.pipe(yielded(hocus_proto::wire::STOP_TABS)));
    assert_eq!(state.chat_tab_focus, Some(state.chat_tab), "the tab bar");

    assert!(state.pipe(yielded(hocus_proto::wire::STOP_INPUT)));
    assert_eq!(state.chat_tab_focus, None, "and the box below it");
    assert!(!state.chat_input_released, "arriving at the box is typing");
}

/// The request is a broadcast - the list holds no id to address it with - so it reaches a hocus
/// the user put in their own layout too. That one is on no ring and must not be handed keys.
#[test]
fn a_yield_is_refused_by_a_tiled_hocus() {
    let mut state = placed_state();
    state.own_is_floating = Some(false);

    assert!(!state.pipe(yielded(hocus_proto::wire::STOP_TABS)));
    assert_eq!(state.chat_tab_focus, None);
}

/// And a Tab may not bring a panel back any more than an Esc may launch one to put away.
#[test]
fn a_yield_does_not_reveal_a_panel_that_is_down() {
    let mut state = placed_state();
    state.hidden = true;

    assert!(!state.pipe(yielded(hocus_proto::wire::STOP_TABS)));
    assert_eq!(state.chat_tab_focus, None);
    assert!(state.hidden, "still down");
}

/// Hiding is an arrival at the conversation like any other: the stop resets, so the panel comes
/// back typing rather than with the bar lit or the box let go of.
#[test]
fn hiding_the_panel_puts_the_box_back_in_editing() {
    let mut state = placed_state();
    state.input.set_text("a draft".to_owned());
    state.on_key(KeyWithModifier::new(BareKey::Esc));
    assert!(state.chat_input_released);

    state.on_key(KeyWithModifier::new(BareKey::Esc));

    assert!(state.hidden);
    assert!(!state.chat_input_released);
    assert_eq!(state.chat_tab_focus, None);
}

/// Our own pane with the keyboard on it. Only the focused-to-unfocused *edge* arms the reset in
/// `on_panes`, so a test about that reset has to start from a manifest that says so.
fn focused_pane(id: u32) -> PaneInfo {
    PaneInfo {
        is_focused: true,
        ..pane(id, false)
    }
}

/// The manifest is a snapshot, and the one taken while the list still had the keyboard is on its
/// way when the yield sets the stop. Believing it would reset the stop the user just asked for
/// and land them in the box instead of on the bar.
#[test]
fn a_stale_manifest_does_not_drop_the_stop_a_yield_just_set() {
    let mut state = placed_state();
    // The keys were here before the ring crossed to the list, and the manifest said so.
    state.on_panes(manifest(0, vec![focused_pane(7)]));

    assert!(state.pipe(yielded(hocus_proto::wire::STOP_TABS)));
    // Taken before our `show_self` landed, delivered after it: still reports us unfocused.
    state.on_panes(manifest(0, vec![pane(7, false)]));

    assert_eq!(
        state.chat_tab_focus,
        Some(state.chat_tab),
        "the bar keeps the highlight the yield gave it"
    );

    // And the focus we asked for arrives, which ends the discount.
    state.on_panes(manifest(0, vec![focused_pane(7)]));
    assert_eq!(state.ring_focus_pending, 0);
    assert_eq!(state.chat_tab_focus, Some(state.chat_tab));
}

/// The discount is for our own request only: once it has landed, leaving is leaving.
#[test]
fn a_leave_after_the_yields_focus_landed_still_resets_the_stop() {
    let mut state = placed_state();
    assert!(state.pipe(yielded(hocus_proto::wire::STOP_TABS)));
    state.on_panes(manifest(0, vec![focused_pane(7)]));

    state.on_panes(manifest(0, vec![pane(7, false)]));

    assert_eq!(
        state.chat_tab_focus, None,
        "a click elsewhere spends the stop"
    );
}

/// And a request that never lands lapses rather than wedging the reset off for good.
#[test]
fn the_discount_runs_out_if_the_focus_never_arrives() {
    let mut state = placed_state();
    assert!(state.pipe(yielded(hocus_proto::wire::STOP_TABS)));
    let budget = state.ring_focus_pending;
    assert!(budget > 0);

    for _ in 0..budget {
        state.on_panes(manifest(0, vec![pane(7, false)]));
    }

    assert_eq!(state.ring_focus_pending, 0);
}

#[test]
fn only_the_client_the_panes_are_sitting_with_may_move_them() {
    // We are on tab 0 with client A. B is on tab 1 and switches to tab 3.
    assert_eq!(
        tab_to_follow(
            Some(TabPosition::new(0)),
            Some(TabPosition::new(1)),
            TabPosition::new(3)
        ),
        None,
        "a client who was never on our tab cannot pull the column onto theirs"
    );
    // A, who is on our tab, switches to tab 3.
    assert_eq!(
        tab_to_follow(
            Some(TabPosition::new(0)),
            Some(TabPosition::new(0)),
            TabPosition::new(3)
        ),
        Some(TabPosition::new(3)),
        "the client we are sitting with takes us along"
    );
}

/// The other half of the loop: the move provokes a fresh report, and that report must not be
/// another move order. `follow_tab` sets `own_tab_index` to where we landed, which is what makes
/// the second look at the same update come back empty.
#[test]
fn a_move_does_not_ask_for_another_one() {
    let (own, was_on, is_on) = (
        Some(TabPosition::new(0)),
        Some(TabPosition::new(0)),
        TabPosition::new(3),
    );
    let moved = tab_to_follow(own, was_on, is_on).expect("the sitting client moves us");
    assert_eq!(
        tab_to_follow(Some(moved), Some(moved), is_on),
        None,
        "the report the move causes re-evaluates to staying put"
    );
}

#[test]
fn nothing_is_followed_until_we_know_where_we_are() {
    assert_eq!(
        tab_to_follow(None, Some(TabPosition::new(0)), TabPosition::new(3)),
        None,
        "`PaneUpdate` is the authority on our own tab, and it has not spoken yet"
    );
    assert_eq!(
        tab_to_follow(Some(TabPosition::new(0)), None, TabPosition::new(3)),
        None,
        "and a client heard from for the first time is not known to have been anywhere"
    );
}

// -- the keyboard leaving the panel -------------------------------------------

/// A tiled pane on the panel's tab, the layer under the overlay.
fn tiled(id: u32, is_focused: bool) -> PaneInfo {
    PaneInfo {
        id: PaneId::Terminal(yinz_tile::prelude::TerminalId::new(id)),
        is_floating: false,
        is_focused,
        ..Default::default()
    }
}

/// A placed panel whose conversation half has the keyboard, as the host last reported it.
fn panel_with_the_keys() -> State {
    let mut state = placed_state();
    state.on_panes(manifest(
        0,
        vec![focused_pane(7), pane(9, false), tiled(3, true)],
    ));
    assert_eq!(state.panel_focus_seen, Some((TabPosition::new(0), true)));
    state
}

/// A click on the pane under the panel, Alt h/j/k/l, or a focus by id: the host takes the
/// keyboard off the Dismissible panel, and the panel goes, as Esc would put it away. Left up it
/// would cover the pane that now has the keys while still marking itself as having them.
#[test]
fn a_lower_layer_taking_the_keyboard_hides_the_panel() {
    let mut state = panel_with_the_keys();
    state.visibility_calls.clear();
    state.keyboard_holder_for_test = Some(tiled(3, true).id);

    assert!(
        state.on_panes(manifest(
            0,
            vec![pane(7, false), pane(9, false), tiled(3, true)]
        )),
        "the hide is drawn"
    );

    assert!(state.hidden);
    assert_eq!(
        state.visibility_calls,
        [(PaneId::Plugin(PluginId::new(7)), false)]
    );
    // And the suppressed manifest the hide causes is not a second one.
    state.visibility_calls.clear();
    assert!(!state.on_panes(manifest(
        0,
        vec![pane(7, true), pane(9, true), tiled(3, true)]
    )));
    assert!(state.visibility_calls.is_empty());
}

/// From the list half as much as from the conversation: the panel is one thing.
#[test]
fn leaving_from_the_list_half_hides_the_panel_too() {
    let mut state = placed_state();
    let list = PaneInfo {
        is_focused: true,
        ..pane(9, false)
    };
    state.on_panes(manifest(0, vec![pane(7, false), list, tiled(3, false)]));
    state.keyboard_holder_for_test = Some(tiled(3, true).id);

    assert!(state.on_panes(manifest(
        0,
        vec![pane(7, false), pane(9, false), tiled(3, true)]
    )));
    assert!(state.hidden);
}

/// Todos `n`, or `Ctrl e n`, opens a modal over the panel, and closing it hands the keyboard
/// back to the panel - which has to still be there to take it.
#[test]
fn a_modal_over_the_panel_does_not_hide_it() {
    let mut state = panel_with_the_keys();
    let modal = PaneInfo {
        id: PaneId::Plugin(PluginId::new(20)),
        is_floating: true,
        is_focused: true,
        ..Default::default()
    };
    state.keyboard_holder_for_test = Some(modal.id);

    state.on_panes(manifest(
        0,
        vec![pane(7, false), pane(9, false), tiled(3, true), modal],
    ));

    assert!(!state.hidden);
}

/// A floating pane that took the keys from the panel gives them to the tiled layer when it
/// closes, not back to the panel. Its time holding them keeps the leave armed, so the tiled
/// pane that has them next puts the panel away as a click on it would have.
#[test]
fn a_floating_pane_taking_the_keys_keeps_the_leave_armed() {
    let mut state = panel_with_the_keys();
    state.visibility_calls.clear();
    let floating = PaneInfo {
        is_floating: true,
        ..tiled(5, true)
    };
    state.keyboard_holder_for_test = Some(floating.id);

    state.on_panes(manifest(
        0,
        vec![pane(7, false), pane(9, false), tiled(3, true), floating],
    ));
    assert!(
        !state.hidden,
        "a floating pane is not the user done with the panel"
    );
    assert_eq!(state.panel_focus_seen, Some((TabPosition::new(0), true)));

    // It closes, and the keys fall to the tiled pane under it.
    state.keyboard_holder_for_test = Some(tiled(3, true).id);
    assert!(
        state.on_panes(manifest(
            0,
            vec![pane(7, false), pane(9, false), tiled(3, true)]
        )),
        "the hide is drawn"
    );
    assert!(state.hidden);
    assert_eq!(
        state.visibility_calls,
        [(PaneId::Plugin(PluginId::new(7)), false)]
    );
}

/// Closing a modal hands the keys back to the panel, which is the panel focused again rather
/// than a leave - and the next tiled leave is one like any other.
#[test]
fn a_modal_handing_the_keys_back_does_not_hide_the_panel() {
    let mut state = panel_with_the_keys();
    let modal = PaneInfo {
        id: PaneId::Plugin(PluginId::new(20)),
        is_floating: true,
        is_focused: true,
        ..Default::default()
    };
    state.keyboard_holder_for_test = Some(modal.id);
    state.on_panes(manifest(
        0,
        vec![pane(7, false), pane(9, false), tiled(3, true), modal],
    ));

    state.keyboard_holder_for_test = Some(PaneId::Plugin(PluginId::new(7)));
    state.on_panes(manifest(
        0,
        vec![focused_pane(7), pane(9, false), tiled(3, true)],
    ));
    assert!(!state.hidden);
    assert_eq!(state.panel_focus_seen, Some((TabPosition::new(0), true)));

    state.keyboard_holder_for_test = Some(tiled(3, true).id);
    assert!(state.on_panes(manifest(
        0,
        vec![pane(7, false), pane(9, false), tiled(3, true)]
    )));
    assert!(state.hidden);
}

/// The New agent modal's hide has no destination, so every panel hears it. The one on the tab
/// the modal was on goes; one on another client's tab is not over the new agent, and stays. A
/// hide that names no tab - the list's Esc - is for this panel wherever it is.
#[test]
fn a_launch_on_another_clients_tab_leaves_the_panel_up() {
    let hide = |tab: Option<&str>| PipeMessage {
        source: PipeSource::Plugin(yinz_tile::prelude::PluginId::new(55)),
        name: hocus_proto::wire::AGENT_STRIP_HIDE.to_owned(),
        payload: None,
        args: tab
            .map(|tab| BTreeMap::from([(hocus_proto::wire::PANEL_TAB.to_owned(), tab.to_owned())]))
            .unwrap_or_default(),
        is_private: true,
        pane_id: None,
    };

    let mut state = placed_state();
    state.pipe(hide(Some("1")));
    assert!(!state.hidden, "a launch over tab 1 is not over this panel");
    assert!(state.visibility_calls.is_empty());

    state.pipe(hide(Some("0")));
    assert!(state.hidden, "one over its own tab puts it away");
    assert_eq!(
        state.visibility_calls,
        [(PaneId::Plugin(PluginId::new(7)), false)]
    );

    let mut state = placed_state();
    state.pipe(hide(None));
    assert!(state.hidden, "and a hide naming no tab always does");
}

/// A leave the host names no holder for is not one to wait on: it disarms, as it always did.
#[test]
fn a_leave_to_nobody_does_not_stay_armed() {
    let mut state = panel_with_the_keys();
    state.keyboard_holder_for_test = None;

    state.on_panes(manifest(
        0,
        vec![pane(7, false), pane(9, false), tiled(3, true)],
    ));
    assert!(!state.hidden);
    assert_eq!(state.panel_focus_seen, Some((TabPosition::new(0), false)));
}

/// Tab across the seam moves the keyboard from one half to the other without it leaving.
#[test]
fn the_ring_crossing_to_the_list_is_not_a_leave() {
    let mut state = panel_with_the_keys();
    // Were the host asked, it would name the pane under the panel: the halves' own flags are
    // what must answer first.
    state.keyboard_holder_for_test = Some(tiled(3, true).id);
    let list = PaneInfo {
        is_focused: true,
        ..pane(9, false)
    };

    state.on_panes(manifest(0, vec![pane(7, false), list, tiled(3, true)]));

    assert!(!state.hidden);
    assert_eq!(state.panel_focus_seen, Some((TabPosition::new(0), true)));
}

/// A visible panel follows the user's tab switch, and lands on the new tab over a tiled pane
/// that has that tab's keyboard. That is the panel coming along, not the user leaving it.
#[test]
fn following_a_tab_switch_is_not_a_leave() {
    let mut state = panel_with_the_keys();
    state.keyboard_holder_for_test = Some(tiled(4, true).id);

    state.on_panes(manifest(
        1,
        vec![pane(7, false), pane(9, false), tiled(4, true)],
    ));

    assert!(!state.hidden);
}

/// A panel that followed a tab switch lands without the keys, but still goes when the
/// keyboard moves on from the pane that had them there, as it would have on the tab it came
/// from. That pane keeping them - a retitle, any manifest after the arrival - is no leave.
#[test]
fn a_panel_that_followed_a_tab_switch_goes_when_the_keyboard_moves_on_its_new_tab() {
    let mut state = panel_with_the_keys();
    state.visibility_calls.clear();
    let tab = |panes| manifest(1, panes);
    state.keyboard_holder_for_test = Some(tiled(4, true).id);
    state.on_panes(tab(vec![
        pane(7, false),
        pane(9, false),
        tiled(4, true),
        tiled(5, false),
    ]));
    assert!(!state.hidden, "arriving is not a leave");
    assert_eq!(state.followed_holder, Some(tiled(4, true).id));
    assert_eq!(state.panel_focus_seen, Some((TabPosition::new(1), true)));

    state.on_panes(tab(vec![
        pane(7, false),
        pane(9, false),
        tiled(4, true),
        tiled(5, false),
    ]));
    assert!(!state.hidden, "the pane that had the keys keeping them");
    assert_eq!(state.panel_focus_seen, Some((TabPosition::new(1), true)));

    state.keyboard_holder_for_test = Some(tiled(5, true).id);
    assert!(
        state.on_panes(tab(vec![
            pane(7, false),
            pane(9, false),
            tiled(4, false),
            tiled(5, true),
        ])),
        "the hide is drawn"
    );
    assert!(
        state.hidden,
        "a tiled pane taking the keys on the new tab puts the panel away"
    );
    assert_eq!(
        state.visibility_calls,
        [(PaneId::Plugin(PluginId::new(7)), false)]
    );
    assert_eq!(state.followed_holder, None);
}

/// Focused after following, the panel is armed as it always is, and the pane it followed onto
/// is a pane like any other: the keys going back to it are a leave.
#[test]
fn a_followed_panel_focused_again_goes_like_any_other() {
    let mut state = panel_with_the_keys();
    state.keyboard_holder_for_test = Some(tiled(4, true).id);
    state.on_panes(manifest(
        1,
        vec![pane(7, false), pane(9, false), tiled(4, true)],
    ));
    state.on_panes(manifest(
        1,
        vec![focused_pane(7), pane(9, false), tiled(4, true)],
    ));
    assert_eq!(state.followed_holder, None);
    assert_eq!(state.panel_focus_seen, Some((TabPosition::new(1), true)));

    state.on_panes(manifest(
        1,
        vec![pane(7, false), pane(9, false), tiled(4, true)],
    ));
    assert!(state.hidden, "a click on the pane it followed onto");
}

/// A second switch before the panel is touched: the manifests in flight name a holder on the
/// tab it is about to follow to, which keeps it armed, and it arrives there armed again with
/// that tab's pane named. With the host naming nobody on arrival, there is no pane to wait on,
/// and it stays up unarmed as it used to.
#[test]
fn a_followed_panel_stays_armed_through_another_switch() {
    let mut state = panel_with_the_keys();
    state.keyboard_holder_for_test = Some(tiled(4, true).id);
    state.on_panes(manifest(
        1,
        vec![pane(7, false), pane(9, false), tiled(4, true)],
    ));

    // The keys are on tab 2 already; the panel is still on tab 1.
    state.keyboard_holder_for_test = Some(tiled(6, true).id);
    state.on_panes(manifest(
        1,
        vec![pane(7, false), pane(9, false), tiled(4, true)],
    ));
    assert!(!state.hidden);
    assert_eq!(state.panel_focus_seen, Some((TabPosition::new(1), true)));

    state.on_panes(manifest(
        2,
        vec![
            pane(7, false),
            pane(9, false),
            tiled(6, true),
            tiled(8, false),
        ],
    ));
    assert!(!state.hidden);
    assert_eq!(state.followed_holder, Some(tiled(6, true).id));
    state.keyboard_holder_for_test = Some(tiled(8, true).id);
    state.on_panes(manifest(
        2,
        vec![
            pane(7, false),
            pane(9, false),
            tiled(6, false),
            tiled(8, true),
        ],
    ));
    assert!(state.hidden);

    let mut state = panel_with_the_keys();
    state.keyboard_holder_for_test = None;
    state.on_panes(manifest(
        1,
        vec![pane(7, false), pane(9, false), tiled(4, true)],
    ));
    assert!(!state.hidden);
    assert_eq!(state.followed_holder, None);
    assert_eq!(state.panel_focus_seen, Some((TabPosition::new(1), false)));
}

/// A modal opened from the pane a followed panel landed over - New agent, `Ctrl e n` - and
/// then cancelled hands the keys back to that pane. That is the pane keeping them, not a tiled
/// pane taking them from the panel: the panel used to go away with the cancelled modal.
#[test]
fn a_modal_cancelled_over_a_followed_panel_leaves_it_up() {
    let mut state = panel_with_the_keys();
    state.keyboard_holder_for_test = Some(tiled(4, true).id);
    state.on_panes(manifest(
        1,
        vec![pane(7, false), pane(9, false), tiled(4, true)],
    ));
    assert_eq!(state.followed_holder, Some(tiled(4, true).id));

    let modal = PaneInfo {
        id: PaneId::Plugin(PluginId::new(20)),
        is_floating: true,
        is_focused: true,
        ..Default::default()
    };
    state.keyboard_holder_for_test = Some(modal.id);
    state.on_panes(manifest(
        1,
        vec![pane(7, false), pane(9, false), tiled(4, false), modal],
    ));
    assert!(!state.hidden);

    state.keyboard_holder_for_test = Some(tiled(4, true).id);
    state.on_panes(manifest(
        1,
        vec![pane(7, false), pane(9, false), tiled(4, true)],
    ));
    assert!(!state.hidden, "the keys back on the pane that had them");

    // The keyboard moving on from that pane still puts the panel away.
    state.keyboard_holder_for_test = Some(tiled(5, true).id);
    state.on_panes(manifest(
        1,
        vec![
            pane(7, false),
            pane(9, false),
            tiled(4, false),
            tiled(5, true),
        ],
    ));
    assert!(state.hidden);
}

/// The host is asked who has the keys only while a leave is armed: not while the panel has
/// them, on each manifest while a floating pane holds them, and not once the panel is away.
#[test]
fn the_host_is_asked_for_the_keyboard_only_while_a_leave_is_armed() {
    let mut state = panel_with_the_keys();
    state.keyboard_holder_calls.set(0);
    state.on_panes(manifest(
        0,
        vec![focused_pane(7), pane(9, false), tiled(3, true)],
    ));
    assert_eq!(state.keyboard_holder_calls.get(), 0, "the panel has them");

    let floating = PaneInfo {
        is_floating: true,
        ..tiled(5, true)
    };
    state.keyboard_holder_for_test = Some(floating.id);
    let with_floating = || {
        manifest(
            0,
            vec![
                pane(7, false),
                pane(9, false),
                tiled(3, true),
                floating.clone(),
            ],
        )
    };
    state.on_panes(with_floating());
    assert_eq!(state.keyboard_holder_calls.get(), 1);
    state.on_panes(with_floating());
    assert_eq!(
        state.keyboard_holder_calls.get(),
        2,
        "asked again while the floating pane holds them"
    );

    state.keyboard_holder_for_test = Some(tiled(3, true).id);
    state.on_panes(manifest(
        0,
        vec![pane(7, false), pane(9, false), tiled(3, true)],
    ));
    assert!(state.hidden);
    assert_eq!(state.keyboard_holder_calls.get(), 3);
    state.on_panes(manifest(
        0,
        vec![pane(7, true), pane(9, true), tiled(3, true)],
    ));
    assert_eq!(
        state.keyboard_holder_calls.get(),
        3,
        "nothing to ask once away"
    );
}

/// A focus the panel asked for itself has manifests in flight from before it, which still say
/// both halves are unfocused. The discount that keeps them from resetting the stop keeps them
/// from hiding the panel too.
#[test]
fn a_manifest_from_before_our_own_focus_is_not_a_leave() {
    let mut state = panel_with_the_keys();
    assert!(state.pipe(yielded(hocus_proto::wire::STOP_TABS)));
    state.keyboard_holder_for_test = Some(tiled(3, true).id);

    state.on_panes(manifest(
        0,
        vec![pane(7, false), pane(9, false), tiled(3, true)],
    ));

    assert!(!state.hidden);
}

/// The chat tab bar's highlight and its help line are drawn from the stop, so a leave that
/// takes the stop with it is a redraw - and a manifest that changes nothing drawn is not.
#[test]
fn on_panes_redraws_when_the_stop_resets() {
    let mut state = placed_state();
    assert!(!state.on_panes(manifest(0, vec![focused_pane(7)])));
    state.chat_tab_focus = Some(state.chat_tab);

    assert!(state.on_panes(manifest(0, vec![pane(7, false)])));
    assert_eq!(state.chat_tab_focus, None);
    assert!(
        !state.hidden,
        "no host to name a holder: the panel stays up"
    );

    assert!(!state.on_panes(manifest(0, vec![pane(7, false)])));
}

// -- Ctrl e o -----------------------------------------------------------------

fn keybind(name: &str) -> PipeMessage {
    PipeMessage {
        source: PipeSource::Keybind,
        name: name.to_owned(),
        payload: None,
        args: BTreeMap::new(),
        is_private: false,
        pane_id: None,
    }
}

/// The agent-mode binding reaches hocus from the keymap, not from the list, so it moves the
/// panel whichever half is up - here a conversation with no list beside it.
#[test]
fn the_keybinding_moves_the_panel_without_a_list() {
    let mut state = placed_state();
    state.agents_plugin = None;
    state.panel.placed_corner = Some(Corner::BottomLeft);

    assert!(!state.pipe(keybind(hocus_proto::wire::AGENT_STRIP_REPOSITION)));

    assert_eq!(state.panel.placed_corner, Some(Corner::TopLeft));
    assert!(
        state.last_placement.as_ref().unwrap().1,
        "at the size it has"
    );
}

/// A panel that is put away stays put, and a hocus in the user's own layout owns no panel.
#[test]
fn the_keybinding_moves_nothing_that_is_not_a_visible_panel() {
    let mut state = placed_state();
    state.hidden = true;
    state.pipe(keybind(hocus_proto::wire::AGENT_STRIP_REPOSITION));
    assert!(state.last_placement.is_none());

    let mut state = placed_state();
    state.own_is_floating = Some(false);
    state.pipe(keybind(hocus_proto::wire::AGENT_STRIP_REPOSITION));
    assert!(state.last_placement.is_none());
}

/// The list's own request still needs the list to be half of this panel.
#[test]
fn the_lists_request_still_needs_the_list_here() {
    let mut state = placed_state();
    state.agents_tab_index = Some(TabPosition::new(1));
    let mut request = keybind(hocus_proto::wire::AGENT_STRIP_REPOSITION);
    request.source = PipeSource::Plugin(PluginId::new(9));

    state.pipe(request);

    assert!(state.last_placement.is_none());
}

// -- the split ----------------------------------------------------------------

/// The container hocus asks for with the configured split `value`, for a panel on a `rows` x
/// `cols` screen, and the panel's own width.
fn configured_request(
    value: &str,
    rows: usize,
    cols: usize,
) -> (yinz_types::multipane::CreateMultipane, usize) {
    let mut state = placed_state();
    state.multipane = None;
    state.surface_rows = rows;
    state.surface_cols = cols;
    state.kdl = config(&[("split", value)]);
    let panel = state.panel_rect().unwrap();
    let PluginCommand::CreateMultipane(request) = state.placement_command(panel, false).unwrap()
    else {
        panic!("create")
    };
    (request, panel.cols)
}

/// The two halves of a `request` as the host draws them on a panel `cols` wide, with the floor
/// the container carries.
fn drawn_halves(
    request: &yinz_types::multipane::CreateMultipane,
    rows: usize,
    cols: usize,
) -> (usize, usize) {
    let outer = yinz_types::pane_size::PaneGeom::from(&Size { rows, cols });
    let ([list, conversation], _) = request.split.geometry(outer, true, request.min_cells);
    (list.cols.as_usize(), conversation.cols.as_usize())
}

/// The configured split `value` as the host would draw it, for a panel on a `rows` x `cols`
/// screen.
fn configured_halves(value: &str, rows: usize, cols: usize) -> (usize, usize) {
    let (request, panel_cols) = configured_request(value, rows, cols);
    drawn_halves(&request, rows, panel_cols)
}

/// "0" and "100%" used to leave one half a single column wide, and "40" on a small screen a
/// ten-column conversation. Each half keeps the conversation's minimum while there is room.
#[test]
fn a_configured_split_leaves_both_halves_room() {
    for (value, rows, cols) in [
        ("0", 40, 120),
        ("0%", 40, 120),
        ("100%", 40, 120),
        ("200%", 40, 120),
        ("40", 40, 80),
    ] {
        let (list, conversation) = configured_halves(value, rows, cols);
        let floor = ui::MIN_COLS.min((list + conversation) / 2);
        assert!(
            list >= floor && conversation >= floor,
            "split {value:?} on {cols} columns gave {list} + {conversation}"
        );
    }
    // Inside the bounds a value is what it says.
    assert_eq!(configured_halves("30", 50, 200).0, 30);
}

/// A share the panel is too narrow for is drawn at the floor, but kept as the share it is: the
/// ratio used to be clamped to the narrow panel's floor, and the list stayed that wide for the
/// rest of the session once the panel grew.
#[test]
fn a_split_set_on_a_narrow_panel_comes_back_when_the_panel_grows() {
    // A 100-column screen gives a panel with too few columns to share for 30% to clear the
    // floor.
    let (request, panel_cols) = configured_request("30%", 40, 100);
    let available = panel_cols - 3;
    assert!(
        available * 3 / 10 < usize::from(request.min_cells),
        "{panel_cols} columns"
    );
    assert_eq!(
        request.split.ratio, 3000,
        "the share asked for, not the floor"
    );
    let (list, _) = drawn_halves(&request, 40, panel_cols);
    assert_eq!(
        list,
        usize::from(request.min_cells),
        "drawn at the floor while narrow"
    );
    // Grown to 120 columns: 117 to share, and 30% of it is back.
    assert_eq!(drawn_halves(&request, 40, 120).0, 117 * 3000 / 10_000);
}

#[test]
fn geometry_warnings_name_each_unparsable_value() {
    for (key, value) in [
        ("width", "abc"),
        ("width", "60.5%"),
        ("height", "-10"),
        ("split", "0.4"),
        ("pad", "5%"),
        ("width", "+"),
    ] {
        let warnings = geometry_warnings(&config(&[(key, value)]));
        assert_eq!(warnings.len(), 1, "{key} {value}");
        assert!(warnings[0].starts_with(&format!("{key} \"{value}\" is not ")));
        assert!(warnings[0].contains("; using "), "{}", warnings[0]);
    }
    assert!(geometry_warnings(&config(&[("split", "0.4")]))[0].ends_with("using 50%"));
    assert!(geometry_warnings(&config(&[("pad", "5%")]))[0].ends_with("using 3"));
    // Clamped, not unreadable - and a blank key is the default. A '+' signs the number it
    // leads, as `usize::from_str` reads it.
    for (key, value) in [
        ("width", "0"),
        ("width", "200%"),
        ("height", "60 %"),
        ("split", "40"),
        ("split", ""),
        ("pad", "20"),
        ("width", "+40"),
        ("split", "+40%"),
    ] {
        assert!(
            geometry_warnings(&config(&[(key, value)])).is_empty(),
            "{key} {value}"
        );
    }
}

/// Edits to other settings must not repeat a notice for the same mistake.
#[test]
fn a_bad_panel_size_is_reported_once_per_mistake() {
    let mut state = placed_state();
    let changed = |state: &mut State, pairs: &[(&str, &str)]| {
        state.handle(Event::PluginConfigurationChanged(config(pairs)))
    };
    let notices = |state: &State| {
        state
            .messages
            .iter()
            .filter(|message| message.content.contains("cannot read a panel setting"))
            .count()
    };

    changed(&mut state, &[("width", "wide")]);
    changed(&mut state, &[("width", "wide")]);
    changed(&mut state, &[("width", "wide"), ("model", "other")]);
    assert_eq!(notices(&state), 1);
    assert!(state
        .messages
        .last()
        .unwrap()
        .content
        .contains("width \"wide\""));

    changed(&mut state, &[("width", "80%")]);
    changed(&mut state, &[("width", "wide")]);
    assert_eq!(notices(&state), 2, "fixed, then broken again");
}

/// An edit to `split` is the user choosing a divider, and moves it. A save that changes
/// anything else must not, or it would undo every drag.
#[test]
fn a_split_edit_moves_the_divider_and_an_unrelated_edit_leaves_it() {
    let mut state = placed_state();

    state.handle(Event::PluginConfigurationChanged(config(&[(
        "split", "25%",
    )])));
    let moved = state.last_split.take().expect("the divider moved");
    assert_eq!(moved.direction, MultipaneDirection::LeftRight);
    assert!((2400..=2500).contains(&moved.ratio), "{}", moved.ratio);

    state.handle(Event::PluginConfigurationChanged(config(&[(
        "split", "25%",
    )])));
    state.handle(Event::PluginConfigurationChanged(config(&[
        ("split", "25%"),
        ("model", "other"),
    ])));
    assert_eq!(state.last_split, None);

    // And the placement the edit also sends still carries no split of its own.
    let PluginCommand::UpdateMultipane(request) = state
        .placement_command(state.panel_rect().unwrap(), false)
        .unwrap()
    else {
        panic!("update")
    };
    assert_eq!(request.split, None);
}

/// `/split` is the keyboard's divider: it moves the split and nothing else, and says what the
/// two halves are now.
#[test]
fn slash_split_moves_only_the_divider() {
    let mut state = placed_state();

    assert!(state.run_manual("/split 25%"));

    assert_eq!(state.last_split.map(|split| split.ratio), Some(2500));
    assert!(state.last_placement.is_none(), "no placement rides along");
    let notice = &state.messages.last().unwrap().content;
    assert!(
        notice.starts_with("split: list 29 · conversation 88"),
        "{notice}"
    );
}

/// A column count is clamped like the configured one, and a bare `/split` only reports.
#[test]
fn slash_split_keeps_both_halves_room_and_a_bare_one_reports() {
    let mut state = placed_state();

    state.run_manual("/split 110");
    let notice = &state.messages.last().unwrap().content;
    assert!(notice.ends_with("conversation 24"), "{notice}");

    state.last_split = None;
    state.run_manual("/split");
    assert_eq!(state.last_split, None);
    let notice = &state.messages.last().unwrap().content;
    assert!(notice.contains("/split 40 or /split 35%"), "{notice}");
}

#[test]
fn a_malformed_split_is_refused_and_sends_nothing() {
    for line in ["/split 0.4", "/split wide", "/split 40%%"] {
        let mut state = placed_state();
        assert!(state.run_manual(line));
        assert_eq!(state.last_split, None, "{line}");
        let error = state.messages.last().unwrap();
        assert_eq!(error.role, crate::message::Role::Error, "{line}");
        assert!(error.content.contains("/split 40"), "{}", error.content);
    }
    let mut state = placed_state();
    state.own_is_floating = Some(false);
    state.run_manual("/split 40");
    assert_eq!(state.last_split, None);
    assert!(state
        .messages
        .last()
        .unwrap()
        .content
        .contains("not the panel"));
}

#[test]
fn global_transparency_opt_in_ignores_old_config_without_reclaiming_user_geometry() {
    let mut state = placed_state();
    for opacity in [0, 75, 100] {
        let outer = state.reported_panel().unwrap();
        state.handle(Event::PluginConfigurationChanged(config(&[(
            "background_opacity",
            &format!("{opacity}%"),
        )])));
        let PluginCommand::UpdateMultipane(update) = state.placement_command(outer, false).unwrap()
        else {
            panic!("update")
        };
        assert_eq!(update.background_opacity, Some(0));
        assert!(!update.override_user_placement);
        assert_eq!(update.split, None);
        assert_eq!(state.reported_panel(), Some(outer));
    }
    state.multipane = None;
    let PluginCommand::CreateMultipane(create) = state
        .placement_command(state.panel_rect().unwrap(), false)
        .unwrap()
    else {
        panic!("create")
    };
    assert_eq!(create.placement.background_opacity, Some(0));
    state.own_is_floating = Some(false);
    assert!(state
        .placement_command(state.panel_rect().unwrap(), false)
        .is_none());
}
