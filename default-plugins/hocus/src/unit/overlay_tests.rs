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
        source: PipeSource::Plugin(zellij_tile::prelude::PluginId::new(7)),
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
