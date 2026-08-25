//! `column_geometry` is a pure function of (config, display area), so it is tested as one.

use super::*;

fn config(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// `(x, y, width, height)` as plain numbers. Every coordinate this module produces is `Fixed`,
/// which is the point - see the note in `column_geometry`.
fn rect(coordinates: &FloatingPaneCoordinates) -> (usize, usize, usize, usize) {
    let fixed = |value: &Option<PercentOrFixed>| match value {
        Some(PercentOrFixed::Fixed(n)) => *n,
        other => panic!("expected a fixed coordinate, got {other:?}"),
    };
    (
        fixed(&coordinates.x),
        fixed(&coordinates.y),
        fixed(&coordinates.width),
        fixed(&coordinates.height),
    )
}

#[test]
fn the_column_sits_in_the_bottom_left_corner_inset_by_the_padding() {
    let (chat, agents) = column_geometry(&BTreeMap::new(), 50, 200).expect("some geometry");
    let (chat_x, chat_y, chat_w, chat_h) = rect(&chat);
    let (agents_x, agents_y, agents_w, agents_h) = rect(&agents);

    assert_eq!(chat_x, DEFAULT_PAD, "left edge is one padding in");
    assert_eq!(agents_x, DEFAULT_PAD, "and the agent list agrees");
    assert_eq!(
        chat_y + chat_h,
        50 - DEFAULT_PAD,
        "the bottom edge is one padding up from the bottom of the screen"
    );
    assert_eq!(
        agents_y, DEFAULT_PAD,
        "and the top of the column is one padding down from the top"
    );
    assert_eq!(chat_w, agents_w, "one column, so one width");
    assert_eq!(chat_w, 50, "25% of 200 columns");
    assert_eq!(
        agents_y + agents_h,
        chat_y,
        "the two halves meet exactly - neither a gap nor an overlap"
    );
}

#[test]
fn the_column_grows_with_the_screen() {
    let (short, _) = column_geometry(&BTreeMap::new(), 30, 100).expect("some geometry");
    let (tall, _) = column_geometry(&BTreeMap::new(), 60, 100).expect("some geometry");
    let (_, short_y, _, short_h) = rect(&short);
    let (_, tall_y, _, tall_h) = rect(&tall);
    assert!(tall_h > short_h, "a taller screen gives a taller column");
    assert_eq!(short_y + short_h, 30 - DEFAULT_PAD);
    assert_eq!(
        tall_y + tall_h,
        60 - DEFAULT_PAD,
        "and the padding off the bottom is the same either way"
    );
}

#[test]
fn width_pad_and_agents_height_are_configurable() {
    let (chat, agents) =
        column_geometry(&config(&[("width", "40"), ("pad", "1")]), 50, 200).expect("some geometry");
    let (chat_x, chat_y, chat_w, chat_h) = rect(&chat);
    assert_eq!(chat_x, 1, "the configured padding");
    assert_eq!(chat_w, 40, "a fixed width is taken as written");
    assert_eq!(chat_y + chat_h, 49, "and the bottom follows the padding");
    assert_eq!(rect(&agents).1, 1, "the top of the column too");

    let (chat, agents) =
        column_geometry(&config(&[("agents_height", "50%")]), 50, 200).expect("some geometry");
    assert_eq!(
        rect(&chat).3,
        rect(&agents).3,
        "an even split gives the two halves the same height"
    );
}

#[test]
fn an_unparseable_value_falls_back_rather_than_crashing() {
    let (chat, _) = column_geometry(&config(&[("width", "wide")]), 50, 200).expect("some geometry");
    let (default_chat, _) = column_geometry(&BTreeMap::new(), 50, 200).expect("some geometry");
    assert_eq!(
        rect(&chat),
        rect(&default_chat),
        "a typo in the KDL costs a default-sized column, not a panic"
    );
}

#[test]
fn the_conversation_keeps_a_drawable_height_on_a_short_screen() {
    // 90% to the agent list would leave the conversation one row
    let (chat, agents) =
        column_geometry(&config(&[("agents_height", "90%")]), 16, 80).expect("some geometry");
    let (_, chat_y, _, chat_h) = rect(&chat);
    let (_, agents_y, _, agents_h) = rect(&agents);
    assert!(
        chat_h >= MIN_HALF_ROWS,
        "the conversation keeps at least a frame and a row, got {chat_h}"
    );
    assert!(
        agents_h >= MIN_HALF_ROWS,
        "and so does the agent list - a zero height would be rejected outright and land the          overlay in the middle of the screen instead"
    );
    assert_eq!(
        agents_y + agents_h,
        chat_y,
        "and the agent list gives up the rows rather than overlapping"
    );
    assert!(chat_y + chat_h <= 16, "the column still fits on the screen");
}

#[test]
fn no_geometry_before_the_first_tab_update() {
    assert!(
        column_geometry(&BTreeMap::new(), 0, 0).is_none(),
        "nothing to compute against yet, and a guess would be seen to move"
    );
    assert!(column_geometry(&BTreeMap::new(), 50, 0).is_none());
}

// ---- hide / reveal transitions ---------------------------------------------------------------
//
// `PaneUpdate` is the only word hocus gets that it was revealed from outside - the `c` keybinding,
// or a tool's `show_self`. It is also a *snapshot*, which is what makes the transition test
// delicate: one taken before our own hide landed still reports us visible.

fn pane(id: u32, is_suppressed: bool) -> PaneInfo {
    PaneInfo {
        id,
        is_plugin: true,
        is_suppressed,
        ..Default::default()
    }
}

fn manifest(tab_index: usize, panes: Vec<PaneInfo>) -> PaneManifest {
    let mut all = std::collections::HashMap::new();
    all.insert(tab_index, panes);
    PaneManifest {
        panes: all,
        ..Default::default()
    }
}

#[test]
fn a_pane_update_reporting_us_suppressed_confirms_the_hide() {
    let mut state = State {
        own_plugin_id: 7,
        hidden: true,
        hide_pending: true,
        ..Default::default()
    };

    state.on_panes(manifest(0, vec![pane(7, true)]));
    assert!(state.hidden);
    assert!(
        !state.hide_pending,
        "the hide we asked for has been seen to land"
    );
    assert_eq!(state.own_tab_index, Some(0), "and we learn where we are");
}

#[test]
fn a_stale_frame_does_not_undo_a_hide() {
    // the user pressed Esc; this snapshot was taken before the instruction reached the server
    let mut state = State {
        own_plugin_id: 7,
        hidden: true,
        hide_pending: true,
        ..Default::default()
    };

    state.on_panes(manifest(0, vec![pane(7, false)]));
    assert!(
        state.hidden,
        "reading a stale frame as a reveal would put the column back the instant Esc was pressed"
    );
    assert!(
        state.hide_pending,
        "and we are still waiting to see the hide"
    );
}

#[test]
fn a_pane_update_after_the_hide_landed_is_a_reveal() {
    let mut state = State {
        own_plugin_id: 7,
        // the hide has been seen to land, so nothing is outstanding
        hidden: true,
        hide_pending: false,
        ..Default::default()
    };

    state.on_panes(manifest(0, vec![pane(7, false)]));
    assert!(
        !state.hidden,
        "something outside put us back - the `c` keybinding, or a tool's show_self"
    );
}

#[test]
fn a_pane_update_while_visible_only_tracks_which_tab_we_are_on() {
    let mut state = State {
        own_plugin_id: 7,
        ..Default::default()
    };

    state.on_panes(manifest(3, vec![pane(9, false), pane(7, false)]));
    assert!(!state.hidden);
    assert_eq!(
        state.own_tab_index,
        Some(3),
        "which is what `follow_tab` needs before it will move anything"
    );
}

#[test]
fn an_agent_list_the_user_closed_is_forgotten() {
    let mut state = State {
        own_plugin_id: 7,
        agents_plugin: Some(9),
        ..Default::default()
    };

    state.on_panes(manifest(0, vec![pane(7, false), pane(9, false)]));
    assert_eq!(state.agents_plugin, Some(9), "still open, still remembered");

    state.on_panes(manifest(0, vec![pane(7, false)]));
    assert_eq!(
        state.agents_plugin, None,
        "otherwise Ctrl-r would go on revealing a pane that is not there, and never open one"
    );
}

#[test]
fn neither_half_collapses_to_nothing_on_a_tiny_screen() {
    // small enough that the two halves cannot both have the rows they would like
    for rows in 6..=14 {
        let (chat, agents) = column_geometry(&BTreeMap::new(), rows, 80)
            .unwrap_or_else(|| panic!("some geometry at {rows} rows"));
        let (_, chat_y, _, chat_h) = rect(&chat);
        let (_, agents_y, _, agents_h) = rect(&agents);
        assert!(
            chat_h > 0 && agents_h > 0,
            "no zero-height half at {rows} rows"
        );
        assert_eq!(
            agents_y + agents_h,
            chat_y,
            "and they still meet exactly at {rows} rows, rather than overlapping"
        );
        assert!(
            chat_y + chat_h <= rows,
            "and the column still fits on screen at {rows} rows"
        );
    }
}

// ---------------------------------------------------------------------------
// Which client's `TabUpdate` is a move order
// ---------------------------------------------------------------------------

/// The loop this rule exists to close, stated as the invariant that closes it: a client who is
/// not on our tab is not asking us to move, so two clients on two tabs have nothing to argue
/// about.
#[test]
fn only_the_client_the_column_is_sitting_with_may_move_it() {
    // We are on tab 0 with client A. B is on tab 1 and switches to tab 3.
    assert_eq!(
        tab_to_follow(Some(0), Some(1), 3),
        None,
        "a client who was never on our tab cannot pull the column onto theirs"
    );
    // A, who is on our tab, switches to tab 3.
    assert_eq!(
        tab_to_follow(Some(0), Some(0), 3),
        Some(3),
        "the client we are sitting with takes us along"
    );
}

/// The other half of the loop: the move provokes a fresh report, and that report must not be
/// another move order. `follow_tab` sets `own_tab_index` to where we landed, which is what makes
/// the second look at the same update come back empty.
#[test]
fn a_move_does_not_ask_for_another_one() {
    let (own, was_on, is_on) = (Some(0), Some(0), 3);
    let moved = tab_to_follow(own, was_on, is_on).expect("the sitting client moves us");
    assert_eq!(
        tab_to_follow(Some(moved), Some(moved), is_on),
        None,
        "the report the move causes re-evaluates to staying put"
    );
}

#[test]
fn nothing_is_followed_until_the_column_knows_where_it_is() {
    assert_eq!(
        tab_to_follow(None, Some(0), 3),
        None,
        "`PaneUpdate` is the authority on our own tab, and it has not spoken yet"
    );
    assert_eq!(
        tab_to_follow(Some(0), None, 3),
        None,
        "and a client heard from for the first time is not known to have been anywhere"
    );
}

/// Two clients, two tabs, the whole exchange - driven through `on_tabs` rather than the rule, so
/// the bookkeeping that feeds it is covered too. A move would panic here (`follow_tab` reaches a
/// value-returning host shim, which reads stdin off wasm), so this is also the strongest possible
/// assertion that no move is attempted.
#[test]
fn two_clients_on_different_tabs_leave_the_column_alone() {
    let mut state = State {
        own_tab_index: Some(0),
        ..State::default()
    };
    let tabs = |active: usize| {
        (0..4)
            .map(|position| TabInfo {
                position,
                tab_id: position,
                active: position == active,
                display_area_rows: 50,
                display_area_columns: 200,
                ..TabInfo::default()
            })
            .collect::<Vec<_>>()
    };

    for _ in 0..3 {
        state.client = 1;
        state.on_tabs(tabs(0));
        state.client = 2;
        state.on_tabs(tabs(1));
    }

    assert_eq!(
        state.own_tab_index,
        Some(0),
        "the column stayed on the tab it started on"
    );
    assert_eq!(
        state.client_tabs,
        BTreeMap::from([(1, 0), (2, 1)]),
        "and both clients were recorded where they actually are"
    );
}

/// Both halves on one tab is the ordinary case, and it must cost nothing: a move here would
/// panic, so this asserts none is attempted as well as that the list's tab is learned.
#[test]
fn the_agent_lists_tab_is_tracked_beside_our_own() {
    let mut state = State {
        own_plugin_id: 7,
        agents_plugin: Some(9),
        ..Default::default()
    };

    state.on_panes(manifest(3, vec![pane(7, false), pane(9, false)]));
    assert_eq!(state.own_tab_index, Some(3));
    assert_eq!(
        state.agents_tab_index,
        Some(3),
        "the pair are together, which is what says there is nothing to rejoin"
    );
}

/// A reveal follows *this* client, not whichever client's `TabUpdate` landed last. With several
/// attached, `tabs`' active flag names nobody in particular - here it still says tab 0, from a
/// client who is not the one being revealed for. Moving on that would be a move to the wrong tab;
/// it would also panic, which is what makes this assertable at all.
#[test]
fn a_reveal_follows_the_client_it_is_revealed_for() {
    let mut state = State {
        own_plugin_id: 7,
        own_tab_index: Some(3),
        hidden: true,
        hide_pending: false,
        client: 2,
        client_tabs: BTreeMap::from([(1, 0), (2, 3)]),
        tabs: vec![TabInfo {
            position: 0,
            tab_id: 0,
            active: true,
            ..TabInfo::default()
        }],
        ..Default::default()
    };

    state.on_panes(manifest(3, vec![pane(7, false)]));
    assert!(!state.hidden, "the reveal was noticed");
    assert_eq!(
        state.own_tab_index,
        Some(3),
        "and the column stayed with the client it was revealed for"
    );
}
