//! `overlay_geometry` is a pure function of (config, display area, whether the chat is on
//! screen), so it is tested as one.

use super::*;

fn config(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// `(x, y, width, height)` as plain numbers. Every coordinate this module produces is `Fixed`,
/// which is the point - see the note in `overlay_geometry`.
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

/// The geometry with the conversation on screen, which is the ordinary case.
fn beside(
    config: &BTreeMap<String, String>,
    rows: usize,
    cols: usize,
) -> (FloatingPaneCoordinates, FloatingPaneCoordinates) {
    overlay_geometry(config, rows, cols, true).expect("some geometry")
}

#[test]
fn the_conversation_sits_in_the_bottom_left_corner_inset_by_the_padding() {
    let (chat, _) = beside(&BTreeMap::new(), 50, 200);
    let (chat_x, chat_y, chat_w, chat_h) = rect(&chat);

    assert_eq!(chat_x, DEFAULT_PAD, "left edge is one padding in");
    assert_eq!(
        chat_y + chat_h,
        50 - DEFAULT_PAD,
        "the bottom edge is one padding up from the bottom of the screen"
    );
    assert_eq!(
        chat_y,
        50 - DEFAULT_PAD - (50 * DEFAULT_HEIGHT_PERCENT / 100),
        "and the top is its height up from that bottom edge - the pane is anchored to the \
         bottom, not stretched between the paddings"
    );
    assert_eq!(chat_w, 60, "30% of 200 columns");
}

/// The whole of the layout change: the list is beside the conversation, not above it.
#[test]
fn the_agent_list_sits_to_the_right_of_the_conversation() {
    let (chat, agents) = beside(&BTreeMap::new(), 50, 200);
    let (chat_x, chat_y, chat_w, chat_h) = rect(&chat);
    let (agents_x, agents_y, agents_w, agents_h) = rect(&agents);

    assert_eq!((agents_y, agents_h), (chat_y, chat_h), "the same slot");
    assert_eq!(agents_w, chat_w, "and the same width");
    assert_eq!(
        agents_x,
        chat_x + chat_w,
        "immediately to its right - each pane draws its own frame, so the two frames meeting \
         is the seam"
    );
}

/// And with the conversation away, the list takes the slot the conversation had: a lone pane
/// belongs in the corner, not floating in the middle of the screen with nothing beside it.
#[test]
fn the_agent_list_takes_the_left_hand_slot_when_the_conversation_is_hidden() {
    let (_, alone) = overlay_geometry(&BTreeMap::new(), 50, 200, false).expect("some geometry");
    let (chat, _) = beside(&BTreeMap::new(), 50, 200);
    let (chat_x, chat_y, _, chat_h) = rect(&chat);
    let (alone_x, alone_y, _, alone_h) = rect(&alone);
    assert_eq!(
        (alone_x, alone_y, alone_h),
        (chat_x, chat_y, chat_h),
        "exactly where the conversation would have been"
    );
}

#[test]
fn the_panes_grow_with_the_screen() {
    let (short, _) = beside(&BTreeMap::new(), 30, 100);
    let (tall, _) = beside(&BTreeMap::new(), 60, 100);
    let (_, short_y, _, short_h) = rect(&short);
    let (_, tall_y, _, tall_h) = rect(&tall);
    assert!(tall_h > short_h, "a taller screen gives a taller pane");
    assert_eq!(short_y + short_h, 30 - DEFAULT_PAD);
    assert_eq!(
        tall_y + tall_h,
        60 - DEFAULT_PAD,
        "and the padding off the bottom is the same either way"
    );
}

#[test]
fn width_and_pad_are_configurable() {
    let (chat, agents) = beside(&config(&[("width", "40"), ("pad", "1")]), 50, 200);
    let (chat_x, chat_y, chat_w, chat_h) = rect(&chat);
    assert_eq!(chat_x, 1, "the configured padding");
    assert_eq!(chat_w, 40, "a fixed width is taken as written");
    assert_eq!(chat_y + chat_h, 49, "and the bottom follows the padding");
    assert_eq!(rect(&agents).0, 41, "the list follows the width across");
}

/// The shape asked for: a panel taller than it is wide, roughly two thirds of the screen's rows
/// and under a third of its columns - not the full-height quarter-width strip hocus used to be,
/// which read as a split of the screen rather than something sitting on top of it.
#[test]
fn a_pane_is_two_thirds_of_the_screen_tall_and_under_a_third_of_it_wide() {
    for (rows, cols) in [(50, 200), (40, 160), (60, 120)] {
        let (chat, _) = beside(&BTreeMap::new(), rows, cols);
        let (_, chat_y, width, height) = rect(&chat);
        assert_eq!(
            height,
            rows * DEFAULT_HEIGHT_PERCENT / 100,
            "two thirds of {rows} rows"
        );
        assert!(
            width * 3 < cols,
            "under a third of {cols} columns, got {width}"
        );
        assert!(
            height > width / 2,
            "and taller than it is wide once a cell's two-to-one aspect is allowed for: \
             {height} rows against {width} columns"
        );
        assert_eq!(
            chat_y,
            rows - DEFAULT_PAD - height,
            "the top floats; the bottom is what is anchored"
        );
    }
}

#[test]
fn the_height_is_configurable_and_kept_on_the_screen() {
    let (chat, agents) = beside(&config(&[("height", "50%")]), 50, 200);
    let (_, chat_y, _, chat_h) = rect(&chat);
    assert_eq!(chat_h, 25, "50% of 50 rows");
    assert_eq!(
        chat_y,
        50 - DEFAULT_PAD - 25,
        "still anchored at the bottom"
    );
    assert_eq!(rect(&agents).3, chat_h, "and the list matches it");

    let (fixed, _) = beside(&config(&[("height", "20")]), 50, 200);
    assert_eq!(rect(&fixed).3, 20, "a fixed height is taken as written");

    // A height nobody could fit is clamped rather than run off the top of the screen, the same
    // way `width` is clamped against the columns.
    let (tall, _) = beside(&config(&[("height", "300%")]), 50, 200);
    let (_, tall_y, _, tall_h) = rect(&tall);
    assert_eq!(
        tall_y, DEFAULT_PAD,
        "clamped to what is between the paddings"
    );
    assert_eq!(tall_y + tall_h, 50 - DEFAULT_PAD);
}

#[test]
fn an_unparseable_value_falls_back_rather_than_crashing() {
    let (chat, _) = beside(&config(&[("width", "wide")]), 50, 200);
    let (default_chat, _) = beside(&BTreeMap::new(), 50, 200);
    assert_eq!(
        rect(&chat),
        rect(&default_chat),
        "a typo in the KDL costs a default-sized pane, not a panic"
    );
}

/// A `width` wide enough for one pane is not wide enough for two, and the pair must not run off
/// the right-hand edge. Alone, the same `width` is allowed the whole of the screen.
#[test]
fn a_width_too_wide_for_two_panes_is_halved_rather_than_overflowed() {
    let cols = 100;
    let config = config(&[("width", "80"), ("pad", "2")]);
    let (chat, agents) = beside(&config, 50, cols);
    let (chat_x, _, chat_w, _) = rect(&chat);
    let (agents_x, _, agents_w, _) = rect(&agents);
    assert_eq!(
        agents_x + agents_w,
        cols - 2,
        "the pair ends exactly at the far padding"
    );
    assert_eq!(chat_x, 2);
    assert_eq!(chat_w, agents_w, "and they split what there is evenly");

    let (_, alone) = overlay_geometry(&config, 50, cols, false).expect("some geometry");
    assert_eq!(
        rect(&alone).2,
        80,
        "a lone pane may have the width it asked for"
    );
}

#[test]
fn a_fat_pad_on_a_narrow_screen_still_leaves_the_panes_on_screen() {
    // `pad` is the x coordinate as well as the inset off the top and bottom, so a cap that looks
    // only at the rows lets a tall, narrow display push a pane off the right-hand edge.
    for cols in [20, 40, 80] {
        for chat_visible in [true, false] {
            let (chat, agents) =
                overlay_geometry(&config(&[("pad", "50")]), 200, cols, chat_visible)
                    .unwrap_or_else(|| panic!("some geometry at {cols} columns"));
            let panes: &[(&str, &FloatingPaneCoordinates)] = match chat_visible {
                true => &[("conversation", &chat), ("agent list", &agents)],
                // the conversation's rectangle is not drawn while it is hidden, so only the
                // list's has to be on screen
                false => &[("agent list", &agents)],
            };
            for (name, rectangle) in panes {
                let (x, y, w, h) = rect(rectangle);
                assert!(
                    x < cols,
                    "the {name}'s left edge is on the screen at {cols} columns, got {x}"
                );
                assert!(
                    x + w <= cols,
                    "and its right edge too at {cols} columns, got {}",
                    x + w
                );
                assert!(
                    y + h <= 200,
                    "and it still fits vertically at {cols} columns, got {}",
                    y + h
                );
            }
        }
    }
}

#[test]
fn no_geometry_before_the_first_tab_update() {
    assert!(
        overlay_geometry(&BTreeMap::new(), 0, 0, true).is_none(),
        "nothing to compute against yet, and a guess would be seen to move"
    );
    assert!(overlay_geometry(&BTreeMap::new(), 50, 0, true).is_none());
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
        "otherwise the geometry pass would go on placing a pane that is not there"
    );
}

// ---- the list's placement, which is tracked apart from the conversation's -------------------
//
// The list is launched by its own keybinding and announces itself from `load`, before the screen
// thread has put its pane on a tab. The placement that announcement makes is dropped in silence,
// exactly as the conversation's first one can be - and `placed`, being about the conversation, was
// already true, so no retry came back to it. That is what left the list at the centered default,
// on top of the conversation.

/// One `AGENT_SELECTION`, as the list sends on load.
fn selection(plugin_id: u32) -> PipeMessage {
    PipeMessage {
        source: PipeSource::Plugin(plugin_id),
        name: hocus_proto::wire::AGENT_SELECTION.to_owned(),
        payload: Some(
            serde_json::to_string(&hocus_proto::wire::Selection {
                plugin_id,
                pane_id: None,
            })
            .expect("serialize"),
        ),
        args: BTreeMap::new(),
        is_private: true,
    }
}

/// A hocus that knows where it is and has nothing to place beside it. `agents_placed` starts true
/// for the same reason `placed` does not: an absent list is not an unplaced one, and a retry that
/// spun on it would fire on every event for as long as nobody opened one.
fn placed_state() -> State {
    let mut state = State {
        own_plugin_id: 7,
        ..Default::default()
    };
    state.on_tabs(vec![TabInfo {
        position: 0,
        tab_id: 0,
        active: true,
        display_area_rows: 50,
        display_area_columns: 200,
        ..TabInfo::default()
    }]);
    state.on_panes(manifest(0, vec![pane(7, false)]));
    assert!(state.placed && state.agents_placed);
    state
}

#[test]
fn a_list_that_announces_itself_before_it_is_on_a_tab_is_placed_when_it_appears() {
    let mut state = placed_state();

    state.pipe(selection(9));
    assert_eq!(state.agents_plugin, Some(9));
    assert!(
        !state.agents_placed,
        "no manifest has named the list's tab, so nothing says that placement reached it"
    );

    state.on_panes(manifest(0, vec![pane(7, false), pane(9, false)]));
    assert!(
        state.agents_placed,
        "the manifest names its tab, so this is the first moment a placement for it can land"
    );

    state.on_panes(manifest(0, vec![pane(7, false), pane(9, false)]));
    assert!(state.agents_placed, "and it stays placed");
}

/// The list sends one of these per `j`/`k` keystroke. None of them is news, and none of them may
/// re-open a placement that has already landed.
#[test]
fn a_repeated_announcement_owes_no_placement() {
    let mut state = placed_state();
    state.agents_plugin = Some(9);
    state.on_panes(manifest(0, vec![pane(7, false), pane(9, false)]));

    state.pipe(selection(9));
    assert!(
        state.agents_placed,
        "the same id it already had, so nothing was announced and nothing is owed"
    );
}

/// A list the user closes and opens again is a *new* pane at the centered default, so the retry
/// has to be owed a second time.
#[test]
fn a_reopened_list_is_placed_again() {
    let mut state = placed_state();
    state.pipe(selection(9));
    state.on_panes(manifest(0, vec![pane(7, false), pane(9, false)]));
    assert!(state.agents_placed);

    state.on_panes(manifest(0, vec![pane(7, false)]));
    assert_eq!(state.agents_plugin, None);
    assert!(
        state.agents_placed,
        "a list that is gone is nothing to place, or the retry would spin"
    );

    state.pipe(selection(11));
    assert!(
        !state.agents_placed,
        "a fresh pane, at the centered default, with a placement owed to it"
    );
    state.on_panes(manifest(0, vec![pane(7, false), pane(11, false)]));
    assert!(state.agents_placed);
}

/// `on_tabs` retries too, and on the same terms: it is the only event that reports the surface,
/// so a list that announced itself between two manifests must not have to wait for a third.
#[test]
fn a_tab_update_retries_the_lists_placement() {
    let mut state = placed_state();
    state.agents_plugin = Some(9);
    state.agents_tab_index = Some(0);
    state.agents_placed = false;

    state.on_tabs(vec![TabInfo {
        position: 0,
        tab_id: 0,
        active: true,
        display_area_rows: 50,
        display_area_columns: 200,
        ..TabInfo::default()
    }]);
    assert!(
        state.agents_placed,
        "the same surface as before, so nothing but `!agents_placed` could have retried this"
    );
}

/// A pane never collapses to nothing: `FloatingPaneCoordinates::new` rejects a zero height, and
/// what comes back in its place is half the surface, *centered* - the pane would jump to the
/// middle of the screen rather than simply being small.
#[test]
fn a_pane_never_collapses_to_nothing_on_a_tiny_screen() {
    for rows in 6..=14 {
        let (chat, agents) = overlay_geometry(&BTreeMap::new(), rows, 80, true)
            .unwrap_or_else(|| panic!("some geometry at {rows} rows"));
        for (name, rectangle) in [("conversation", &chat), ("agent list", &agents)] {
            let (_, y, _, h) = rect(rectangle);
            assert!(h > 0, "no zero-height {name} at {rows} rows");
            assert!(
                y + h <= rows,
                "and the {name} still fits on screen at {rows} rows"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Which client's `TabUpdate` is a move order
// ---------------------------------------------------------------------------

/// The loop this rule exists to close, stated as the invariant that closes it: a client who is
/// not on our tab is not asking us to move, so two clients on two tabs have nothing to argue
/// about.
#[test]
fn only_the_client_the_panes_are_sitting_with_may_move_them() {
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
fn nothing_is_followed_until_we_know_where_we_are() {
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
fn two_clients_on_different_tabs_leave_the_panes_alone() {
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

/// The column is placed even when the only `TabUpdate` that reports a change of display area
/// arrives before the server knows where our pane is.
///
/// That `TabUpdate` is where the placement used to happen and nowhere else, and a placement made
/// then is dropped in silence: `Screen::change_overlay_coordinates` looks the pane up in the tabs
/// and does nothing when no tab contains it - the state a `TabUpdate` replayed from the
/// pre-permission cache arrives in. No later update reports a *change*, so the column was left at
/// the centered half-screen the host gives an overlay that named no coordinates.
#[test]
fn a_placement_that_could_not_land_is_made_again_when_the_pane_appears() {
    let mut state = State {
        own_plugin_id: 7,
        ..Default::default()
    };
    let tabs = vec![TabInfo {
        position: 0,
        tab_id: 0,
        active: true,
        display_area_rows: 50,
        display_area_columns: 200,
        ..TabInfo::default()
    }];

    state.on_tabs(tabs.clone());
    assert_eq!(
        (state.surface_rows, state.surface_cols),
        (50, 200),
        "the surface's size is learned here whether or not the placement lands"
    );
    assert!(
        !state.placed,
        "nothing said our pane was on a tab, so nothing says the placement reached it"
    );

    state.on_panes(manifest(0, vec![pane(7, false)]));
    assert!(
        state.placed,
        "the manifest names our tab, so this is the first moment a placement can land - and does"
    );

    state.on_tabs(tabs);
    assert!(state.placed, "and it stays placed");
}

/// A reveal stays on the tab the manifest found us on, whichever client's copy of that manifest
/// arrives first.
///
/// The host builds one `PaneUpdate` per attached client and sends every one of them, so the copy
/// that happens to be processed first names an arbitrary client. This one is driven as client 1,
/// who is recorded on tab 0 while the pane is on tab 3 - the shape that used to drag the column
/// off the summoner's screen. Following tab 0 would also panic, which is what makes "no move was
/// attempted" assertable at all.
#[test]
fn a_reveal_stays_on_the_tab_the_server_put_us_on() {
    let mut state = State {
        own_plugin_id: 7,
        own_tab_index: Some(3),
        hidden: true,
        hide_pending: false,
        client: 1,
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
        "and the column stayed where `move_to_focused_tab` had already put it"
    );
}

// ---------------------------------------------------------------------------
// The two panes hide and show on their own
// ---------------------------------------------------------------------------

/// The separation, from the conversation's side: a list revealed on its own - by `a`, or by the
/// user - leaves a hidden conversation hidden. It used to bring the conversation back with it,
/// which is exactly the coupling the panes no longer have.
#[test]
fn a_visible_agent_list_does_not_bring_a_hidden_conversation_back() {
    let mut state = State {
        own_plugin_id: 7,
        agents_plugin: Some(9),
        own_tab_index: Some(3),
        hidden: true,
        hide_pending: false,
        ..Default::default()
    };

    state.on_panes(manifest(3, vec![pane(7, true), pane(9, false)]));

    assert!(state.hidden, "the conversation stays where the user put it");
    assert!(
        state.agents_visible,
        "and the list beside it is noted as drawn, so a tab switch carries it"
    );
    assert_eq!(state.own_tab_index, Some(3));
}

/// The same arm read the other way: our own hide landing is a hide, whatever the list is doing.
#[test]
fn our_own_hide_landing_is_not_a_reveal() {
    let mut state = State {
        own_plugin_id: 7,
        agents_plugin: Some(9),
        hidden: true,
        hide_pending: true,
        ..Default::default()
    };

    state.on_panes(manifest(3, vec![pane(7, true), pane(9, true)]));

    assert!(state.hidden, "we are away, and stay away");
    assert!(
        !state.hide_pending,
        "the hide we asked for has been seen to land"
    );
}

// ---------------------------------------------------------------------------
// A conversation the agent list launched
// ---------------------------------------------------------------------------

/// The list needs a hocus - it draws rows only hocus computes - but the user who opened a list
/// asked for a list. A greeting arriving before our own first `PaneUpdate` is the launch that
/// made us, and nothing else can be that early.
#[test]
fn a_greeting_that_arrives_before_our_first_pane_update_means_we_were_launched_hidden() {
    let mut state = State {
        own_plugin_id: 7,
        ..Default::default()
    };
    assert!(state.own_tab_index.is_none(), "no manifest yet");

    state.note_greeting_from_agents(true);
    assert!(state.open_hidden);
}

/// And a hocus that was already open simply answers with the strip: the greeting is the list
/// saying hello, not the list having opened us.
#[test]
fn a_greeting_to_a_conversation_that_was_already_open_hides_nothing() {
    let mut state = State {
        own_plugin_id: 7,
        own_tab_index: Some(0),
        ..Default::default()
    };

    state.note_greeting_from_agents(true);
    assert!(!state.open_hidden);

    // nor does a greeting with no marker on it at all
    let mut state = State {
        own_plugin_id: 7,
        ..Default::default()
    };
    state.note_greeting_from_agents(false);
    assert!(!state.open_hidden);
}

/// The hide itself waits for a pane to hide: `hide_self` before our pane is on a tab is dropped
/// without a word, the same way a placement is.
#[test]
fn the_hide_lands_on_the_first_manifest_that_finds_our_pane() {
    let mut state = State {
        own_plugin_id: 7,
        open_hidden: true,
        ..Default::default()
    };

    state.on_panes(manifest(0, vec![pane(7, false)]));

    assert!(
        state.hidden,
        "put away as soon as there was something to put"
    );
    assert!(state.hide_pending);
    assert!(!state.open_hidden, "and asked for exactly once");
}

// ---- which rectangle the panes are placed against ---------------------------------------------
//
// `TabInfo` reports two of them and they are not the same: `display_area_*` is the tab centre,
// what is left once the global-pane strips have been carved off, and `session_surface_*` is the
// whole surface those strips were carved out of. The server resolves an overlay's coordinates
// against the second, so that is the one the arithmetic has to use.

/// A tab whose centre is smaller than the surface by a strip on each axis - the shape any session
/// with a global pane in it has.
fn tab_with_a_strip() -> TabInfo {
    TabInfo {
        position: 0,
        tab_id: 0,
        active: true,
        display_area_rows: 45,
        display_area_columns: 190,
        session_surface_rows: 50,
        session_surface_columns: 200,
        ..TabInfo::default()
    }
}

#[test]
fn the_panes_are_placed_against_the_session_surface_and_not_the_tab() {
    let tab = tab_with_a_strip();
    assert_eq!(
        surface_of(&tab),
        (50, 200),
        "the surface, not the centre the strips left behind"
    );

    let (rows, cols) = surface_of(&tab);
    let (chat, _) = beside(&BTreeMap::new(), rows, cols);
    let (_, chat_y, chat_w, chat_h) = rect(&chat);
    assert_eq!(
        chat_y + chat_h,
        50 - DEFAULT_PAD,
        "the bottom edge is one padding up from the bottom of the *surface* - placing against the \
         tab centre would add the strip's rows to the padding the user asked for"
    );
    assert_eq!(
        chat_w, 60,
        "and the width is 30% of the surface, not of the centre"
    );
}

/// A `TabInfo` from a server that predates the field. Wrong by the strips, which is what the code
/// did before - and much better than a pane collapsed to nothing.
#[test]
fn a_tab_info_without_a_surface_falls_back_to_the_display_area() {
    let tab = TabInfo {
        display_area_rows: 45,
        display_area_columns: 190,
        ..TabInfo::default()
    };
    assert_eq!(surface_of(&tab), (45, 190));
}

/// Both-or-neither: half a surface is not a surface, and mixing the two denominators is exactly
/// the bug this pair of fields exists to close.
#[test]
fn a_half_populated_surface_falls_back_as_a_whole() {
    let rows_only = TabInfo {
        display_area_rows: 45,
        display_area_columns: 190,
        session_surface_rows: 50,
        ..TabInfo::default()
    };
    assert_eq!(surface_of(&rows_only), (45, 190));

    let cols_only = TabInfo {
        display_area_rows: 45,
        display_area_columns: 190,
        session_surface_columns: 200,
        ..TabInfo::default()
    };
    assert_eq!(surface_of(&cols_only), (45, 190));
}

#[test]
fn the_surface_is_what_a_tab_update_caches() {
    let mut state = State {
        own_plugin_id: 7,
        ..Default::default()
    };
    state.on_tabs(vec![tab_with_a_strip()]);
    assert_eq!((state.surface_rows, state.surface_cols), (50, 200));
}

// ---- putting a parted pair back together -----------------------------------------------------
//
// Both keybindings carry `move_to_focused_tab`, and both move only the half they name. Which half
// moved is the whole question, and the answer is which half the server focused.

#[test]
fn a_summoned_agent_list_is_followed_rather_than_dragged_back() {
    assert_eq!(rejoin(0, 1, true, true), Some(Rejoin::Follow(1)));
}

#[test]
fn an_agent_list_left_behind_by_a_summoned_conversation_is_dragged_over() {
    assert_eq!(rejoin(1, 0, false, true), Some(Rejoin::Drag(1)));
}

/// Focus outlives the client that set it, so a list focused and then abandoned on a tab nobody is
/// looking at must not get to pull the column onto it.
#[test]
fn a_focused_agent_list_on_a_tab_nobody_is_on_is_still_dragged_over() {
    assert_eq!(rejoin(0, 1, true, false), Some(Rejoin::Drag(0)));
}

#[test]
fn a_pair_that_is_already_together_asks_for_nothing() {
    for focused in [true, false] {
        for someones in [true, false] {
            assert_eq!(rejoin(2, 2, focused, someones), None);
        }
    }
}

// `rejoin` is tested as the pure function it is, and deliberately not through `on_panes`: both of
// its arms end in `break_panes_to_tab_with_index`, a value-returning host call that blocks on a
// reply no native test can send. That is the same reason `tab_to_follow` is a free function - the
// decision is the part worth pinning, and it is the part that can be.

// ---- migrating a legacy global-pane hocus ------------------------------------------------------
//
// A session dump written before this column became a pair of overlays restores both halves as
// *global panes*. Overlays are not serialized, so nothing else brings the column back and nothing
// else takes the old strip away: the keybinding opens a second, overlay column beside it.

/// A manifest in which the server is drawing us as a global pane - the restored shape.
fn global_manifest(own_id: u32) -> PaneManifest {
    PaneManifest {
        global_panes: vec![pane(own_id, false)],
        ..Default::default()
    }
}

#[test]
fn a_hocus_restored_as_a_global_pane_hands_over_exactly_once() {
    let mut state = State {
        own_plugin_id: 7,
        legacy_suspect: true,
        ..Default::default()
    };

    state.on_panes(global_manifest(7));
    assert!(
        state.migrating,
        "the manifest confirms the suspicion the configuration raised"
    );

    // A second manifest before the close lands must not send a second handover: two launches are
    // two chances to disagree about the configuration the lookup matches on.
    state.on_panes(global_manifest(7));
    assert!(state.migrating);
    assert!(
        state.legacy_suspect,
        "and this half stays a suspect for as long as it is here - it is on its way out, not \
         carrying on as an ordinary column"
    );
}

/// The suspicion is raised by a missing `overlay` key, which is also true of a hocus someone put
/// in a layout on purpose. The manifest is what tells the two apart, and clearing the suspicion is
/// what lets the deferred load-time work happen.
#[test]
fn a_non_overlay_hocus_that_is_not_a_global_pane_carries_on() {
    let mut state = State {
        own_plugin_id: 7,
        legacy_suspect: true,
        ..Default::default()
    };

    state.on_panes(manifest(2, vec![pane(7, false)]));

    assert!(!state.migrating, "nothing to hand over to");
    assert!(!state.legacy_suspect, "and nothing left to suspect");
    assert_eq!(
        state.own_tab_index,
        Some(2),
        "the ordinary bookkeeping runs, rather than being skipped by an early return"
    );
}
