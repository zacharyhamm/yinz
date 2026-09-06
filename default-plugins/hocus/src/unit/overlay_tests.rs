//! `overlay_rects` is a pure function of (config, display area, which halves are on screen,
//! the split), so it is tested as one.

use super::*;

fn config(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// `(x, y, width, height)` as plain numbers. Every coordinate this module produces is `Fixed`,
/// which is the point - see the note in `fixed_rect`.
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

/// The panel at home - never moved by a press - as the coordinates the host takes, in the order
/// `(chat, agents)`.
///
/// `overlay_rects` is the production arithmetic and `fixed_rect` the conversion every placement
/// makes per half; this does both at once, so a test can compare the pair.
fn home_geometry(
    config: &BTreeMap<String, String>,
    rows: usize,
    cols: usize,
    shown: Shown,
) -> Option<(FloatingPaneCoordinates, FloatingPaneCoordinates)> {
    let (chat, agents) = overlay_rects(config, rows, cols, shown, None)?;
    Some((fixed_rect(chat), fixed_rect(agents)))
}

/// The geometry with both halves on screen, which is the ordinary case.
fn beside(
    config: &BTreeMap<String, String>,
    rows: usize,
    cols: usize,
) -> (FloatingPaneCoordinates, FloatingPaneCoordinates) {
    home_geometry(config, rows, cols, Shown::BOTH).expect("some geometry")
}

/// One half on screen.
fn only_chat() -> Shown {
    Shown {
        chat: true,
        agents: false,
    }
}
fn only_agents() -> Shown {
    Shown {
        chat: false,
        agents: true,
    }
}

/// The panel the two halves make: the rectangle round both.
fn panel_of(
    chat: &FloatingPaneCoordinates,
    agents: &FloatingPaneCoordinates,
) -> (usize, usize, usize, usize) {
    let (chat_x, chat_y, chat_w, chat_h) = rect(chat);
    let (agents_x, agents_y, agents_w, agents_h) = rect(agents);
    let x = chat_x.min(agents_x);
    let y = chat_y.min(agents_y);
    (
        x,
        y,
        (chat_x + chat_w).max(agents_x + agents_w) - x,
        (chat_y + chat_h).max(agents_y + agents_h) - y,
    )
}

#[test]
fn the_panel_sits_in_the_bottom_left_corner_inset_by_the_padding() {
    let (chat, agents) = beside(&BTreeMap::new(), 50, 200);
    let (x, y, w, h) = panel_of(&chat, &agents);

    assert_eq!(x, DEFAULT_PAD, "left edge is one padding in");
    assert_eq!(
        y + h,
        50 - DEFAULT_PAD,
        "the bottom edge is one padding up from the bottom of the screen"
    );
    assert_eq!(
        y,
        50 - DEFAULT_PAD - (50 * DEFAULT_HEIGHT_PERCENT / 100),
        "and the top is its height up from that bottom edge - the panel is anchored to the \
         bottom, not stretched between the paddings"
    );
    assert_eq!(w, 120, "60% of 200 columns");
}

/// The whole of the layout: one panel, the list on the left and the conversation on the right,
/// with nothing between them - the two touching frames are the divider.
#[test]
fn the_agent_list_is_the_left_half_and_the_conversation_the_right() {
    let (chat, agents) = beside(&BTreeMap::new(), 50, 200);
    let (chat_x, chat_y, chat_w, chat_h) = rect(&chat);
    let (agents_x, agents_y, agents_w, agents_h) = rect(&agents);

    assert_eq!((agents_y, agents_h), (chat_y, chat_h), "the same rows");
    assert_eq!(
        agents_x, DEFAULT_PAD,
        "the list is at the panel's left edge"
    );
    assert_eq!(
        chat_x,
        agents_x + agents_w,
        "and the conversation starts on the column the list ends on"
    );
    assert_eq!((agents_w, chat_w), (60, 60), "cut in half by default");
}

/// `split` is the list's share, as a percentage of the panel or a column count, and the
/// conversation gets the rest - the panel's width does not change with it.
#[test]
fn the_split_is_configurable() {
    for (split, agents_cols) in [("25%", 30), ("75%", 90), ("40", 40)] {
        let (chat, agents) = beside(&config(&[("split", split)]), 50, 200);
        let (agents_x, _, agents_w, _) = rect(&agents);
        let (chat_x, _, chat_w, _) = rect(&chat);
        assert_eq!(agents_w, agents_cols, "split {split}");
        assert_eq!(chat_x, agents_x + agents_w, "split {split}");
        assert_eq!(
            agents_w + chat_w,
            120,
            "split {split}: the panel is still 120 wide"
        );
    }
}

/// Each half keeps the minimum, whichever way the split leans: a `split` that would fold one
/// half to nothing is pulled back to the floor.
#[test]
fn the_split_is_floored_on_both_sides() {
    let (chat, agents) = beside(&config(&[("split", "1")]), 50, 200);
    assert_eq!(rect(&agents).2, MIN_PANE_COLS);
    assert_eq!(rect(&chat).2, 120 - MIN_PANE_COLS);

    let (chat, agents) = beside(&config(&[("split", "100%")]), 50, 200);
    assert_eq!(rect(&chat).2, MIN_PANE_COLS);
    assert_eq!(rect(&agents).2, 120 - MIN_PANE_COLS);
}

/// A seam drag reaches the placement as a ratio, and the ratio is what a re-cut honours: the
/// same panel is cut at the same column, and a wider panel in the same proportion.
#[test]
fn a_split_read_off_the_manifest_outranks_the_configured_one() {
    let config = config(&[("split", "25%")]);
    let (chat, agents) =
        overlay_rects(&config, 50, 200, Shown::BOTH, Some((90, 120))).expect("some geometry");
    assert_eq!(
        (agents.cols, chat.cols),
        (90, 30),
        "cut where the user dragged it"
    );
    assert_eq!(chat.x, agents.x + agents.cols);

    // the terminal grew: 60% of 300 is a 180-column panel, cut three to one as before
    let (chat, agents) =
        overlay_rects(&config, 50, 300, Shown::BOTH, Some((90, 120))).expect("some geometry");
    assert_eq!(
        (agents.cols, chat.cols),
        (135, 45),
        "the proportion, not the column"
    );
}

/// A half with no sibling to share with takes the whole panel: the panel keeps one footprint
/// whether it holds one half or two. The absent half is still placed, at the share it would
/// have, so a panel that grows its other half finds it where it belongs.
#[test]
fn a_lone_half_takes_the_whole_panel() {
    let (both_chat, both_agents) = beside(&BTreeMap::new(), 50, 200);
    let panel = panel_of(&both_chat, &both_agents);

    let (chat, agents) =
        home_geometry(&BTreeMap::new(), 50, 200, only_chat()).expect("some geometry");
    assert_eq!(
        rect(&chat),
        panel,
        "the conversation alone has the whole panel"
    );
    assert_eq!(
        rect(&agents),
        rect(&both_agents),
        "the absent list keeps its half, ready to arrive into it"
    );

    let (chat, agents) =
        home_geometry(&BTreeMap::new(), 50, 200, only_agents()).expect("some geometry");
    assert_eq!(rect(&agents), panel, "the list alone has the whole panel");
    assert_eq!(rect(&chat), rect(&both_chat));

    // and with neither half named the panel is cut as usual - which is what a hidden panel is
    // placed as, since hiding takes both halves and gives the width back to nobody
    let neither = Shown {
        chat: false,
        agents: false,
    };
    let (chat, agents) = home_geometry(&BTreeMap::new(), 50, 200, neither).expect("some geometry");
    assert_eq!(
        (rect(&chat), rect(&agents)),
        (rect(&both_chat), rect(&both_agents))
    );
}

#[test]
fn the_panel_grows_with_the_screen() {
    let (short, _) = beside(&BTreeMap::new(), 30, 100);
    let (tall, _) = beside(&BTreeMap::new(), 60, 100);
    let (_, short_y, _, short_h) = rect(&short);
    let (_, tall_y, _, tall_h) = rect(&tall);
    assert!(tall_h > short_h, "a taller screen gives a taller panel");
    assert_eq!(short_y + short_h, 30 - DEFAULT_PAD);
    assert_eq!(
        tall_y + tall_h,
        60 - DEFAULT_PAD,
        "and the padding off the bottom is the same either way"
    );
}

#[test]
fn width_and_pad_are_configurable() {
    let (chat, agents) = beside(&config(&[("width", "80"), ("pad", "1")]), 50, 200);
    let (x, y, w, h) = panel_of(&chat, &agents);
    assert_eq!(x, 1, "the configured padding");
    assert_eq!(
        w, 80,
        "a fixed width is taken as written, and is the whole panel's"
    );
    assert_eq!(y + h, 49, "and the bottom follows the padding");
    assert_eq!(rect(&agents).2, 40, "the list has half of it");
    assert_eq!(
        rect(&chat).0,
        41,
        "and the conversation starts where it ends"
    );
}

/// The shape asked for: a panel taller than either half is wide, roughly two thirds of the
/// screen's rows and just over half its columns.
#[test]
fn the_panel_is_two_thirds_of_the_screen_tall_and_sixty_percent_of_it_wide() {
    for (rows, cols) in [(50, 200), (40, 160), (60, 120)] {
        let (chat, agents) = beside(&BTreeMap::new(), rows, cols);
        let (_, y, width, height) = panel_of(&chat, &agents);
        assert_eq!(
            height,
            rows * DEFAULT_HEIGHT_PERCENT / 100,
            "two thirds of {rows} rows"
        );
        assert_eq!(
            width,
            cols * DEFAULT_WIDTH_PERCENT / 100,
            "60% of {cols} columns"
        );
        assert_eq!(
            y,
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
    let (chat, agents) = beside(&config(&[("width", "wide"), ("split", "half")]), 50, 200);
    let (default_chat, default_agents) = beside(&BTreeMap::new(), 50, 200);
    assert_eq!(
        (rect(&chat), rect(&agents)),
        (rect(&default_chat), rect(&default_agents)),
        "a typo in the KDL costs a default-sized panel, not a panic"
    );
}

/// A `width` wider than the screen leaves is clamped to what the padding leaves, and the halves
/// share whatever the panel gets.
#[test]
fn a_width_too_wide_for_the_screen_is_clamped_rather_than_overflowed() {
    let cols = 100;
    let config = config(&[("width", "300"), ("pad", "2")]);
    let (chat, agents) = beside(&config, 50, cols);
    let (x, _, w, _) = panel_of(&chat, &agents);
    assert_eq!(x + w, cols - 2, "the panel ends exactly at the far padding");
    assert_eq!(x, 2);
    assert_eq!(
        rect(&chat).2,
        rect(&agents).2,
        "and the halves split what there is evenly"
    );
}

#[test]
fn a_fat_pad_on_a_narrow_screen_still_leaves_the_panel_on_screen() {
    // `pad` is the x coordinate as well as the inset off the top and bottom, so a cap that looks
    // only at the rows lets a tall, narrow display push a panel off the right-hand edge.
    for cols in [20, 40, 80] {
        for shown in [Shown::BOTH, only_chat(), only_agents()] {
            let (chat, agents) = home_geometry(&config(&[("pad", "50")]), 200, cols, shown)
                .unwrap_or_else(|| panic!("some geometry at {cols} columns"));
            let mut panes: Vec<(&str, &FloatingPaneCoordinates)> = vec![];
            if shown.chat {
                panes.push(("conversation", &chat));
            }
            if shown.agents {
                panes.push(("agent list", &agents));
            }
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
        home_geometry(&BTreeMap::new(), 0, 0, Shown::BOTH).is_none(),
        "nothing to compute against yet, and a guess would be seen to move"
    );
    assert!(home_geometry(&BTreeMap::new(), 50, 0, Shown::BOTH).is_none());
}

// ---- hide / reveal transitions ---------------------------------------------------------------
//
// `PaneUpdate` is the only word hocus gets that it was revealed from outside - the `c` keybinding,
// or a tool's `show_self`. It is also a *snapshot*, which is what makes the transition test
// delicate: one taken before our own hide landed still reports us visible.

/// Our own pane as the manifest reports it: an overlay, which is what makes hocus a panel at all.
///
/// `is_floating` is the whole of that - an overlay reports it unconditionally and a layout pane
/// never does - so a fixture that left it at `Default` would be describing a tiled hocus, which
/// grows no list and places nothing. See `State::is_panel`.
fn pane(id: u32, is_suppressed: bool) -> PaneInfo {
    PaneInfo {
        id,
        is_plugin: true,
        is_floating: true,
        is_suppressed,
        ..Default::default()
    }
}

fn manifest(tab_index: usize, panes: Vec<PaneInfo>) -> PaneManifest {
    let mut all = std::collections::HashMap::new();
    all.insert(tab_index, panes);
    PaneManifest { panes: all }
}

#[test]
fn a_pane_update_reporting_us_suppressed_confirms_the_hide() {
    let mut state = State {
        own_is_floating: Some(true),
        own_plugin_id: 7,
        hidden: true,
        hide_pending: 2,
        ..Default::default()
    };

    state.on_panes(manifest(0, vec![pane(7, true)]));
    assert!(state.hidden);
    assert!(
        state.hide_pending == 0,
        "the hide we asked for has been seen to land"
    );
    assert_eq!(state.own_tab_index, Some(0), "and we learn where we are");
}

#[test]
fn a_stale_frame_does_not_undo_a_hide() {
    // the user pressed Esc; this snapshot was taken before the instruction reached the server
    let mut state = State {
        own_is_floating: Some(true),
        own_plugin_id: 7,
        hidden: true,
        hide_pending: 2,
        ..Default::default()
    };

    state.on_panes(manifest(0, vec![pane(7, false)]));
    assert!(
        state.hidden,
        "reading a stale frame as a reveal would put the column back the instant Esc was pressed"
    );
    assert!(
        state.hide_pending > 0,
        "and we are still waiting to see the hide"
    );
}

#[test]
fn a_pane_update_after_the_hide_landed_is_a_reveal() {
    let mut state = State {
        own_is_floating: Some(true),
        own_plugin_id: 7,
        // the hide has been seen to land, so nothing is outstanding
        hidden: true,
        hide_pending: 0,
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
        own_is_floating: Some(true),
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
        own_is_floating: Some(true),
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
/// An `AGENT_STRIP_YIELD` naming `stop`, as the agent list sends it when the ring steps out.
fn yielded(stop: &str) -> PipeMessage {
    PipeMessage {
        source: PipeSource::Plugin(7),
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

fn selection(plugin_id: u32) -> PipeMessage {
    PipeMessage {
        source: PipeSource::Plugin(plugin_id),
        name: hocus_proto::wire::AGENT_SELECTION.to_owned(),
        payload: Some(
            serde_json::to_string(&hocus_proto::wire::Selection {
                plugin_id,
                pane_id: None,
                run_id: None,
            })
            .expect("serialize"),
        ),
        args: BTreeMap::new(),
        is_private: true,
        pane_id: None,
    }
}

/// A hocus that knows where it is and has nothing to place beside it. `agents_placed` starts true
/// for the same reason `placed` does not: an absent list is not an unplaced one, and a retry that
/// spun on it would fire on every event for as long as nobody opened one.
fn placed_state() -> State {
    let mut state = State {
        own_is_floating: Some(true),
        own_plugin_id: 7,
        kdl: state_config(),
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

/// The config every state-level test runs under: a 60-column panel on the standard 50x200
/// surface. Narrower than the default so that a panel can be clear of a pane that has half the
/// screen, which is what the Ctrl-o fixtures below are about; two 30-column halves when the list
/// is open, which is above the floor.
fn state_config() -> BTreeMap<String, String> {
    config(&[("width", "60")])
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

/// A resize that lands while *both* halves are set aside still has to reach the geometry.
///
/// Every `TabUpdate` caches the surface whether or not anything is on screen, and it is the only
/// event that reports one - so a resize skipped because nothing was drawn is never re-reported,
/// and the pane the user reveals afterwards comes back sized to a screen that is gone.
#[test]
fn a_resize_while_both_halves_are_hidden_is_still_placed() {
    let mut state = placed_state();
    state.pipe(selection(9));
    state.on_panes(manifest(0, vec![pane(7, false), pane(9, false)]));
    assert_eq!(state.placed_against, (50, 200));

    // both away: the conversation on Esc, the list on its own binding
    state.on_panes(manifest(0, vec![pane(7, true), pane(9, true)]));
    assert!(state.hidden && !state.agents_visible);

    state.on_tabs(vec![TabInfo {
        position: 0,
        tab_id: 0,
        active: true,
        display_area_rows: 25,
        display_area_columns: 100,
        ..TabInfo::default()
    }]);
    assert_eq!(
        state.placed_against,
        (25, 100),
        "the surface was cached, so the placement it implies cannot be skipped for being invisible"
    );

    state.on_panes(manifest(0, vec![pane(7, true), pane(9, false)]));
    assert!(state.agents_visible);
    assert_eq!(
        state.placed_against,
        (25, 100),
        "and the list comes back to a geometry that is already the new screen's"
    );
}

/// A pane never collapses to nothing: `FloatingPaneCoordinates::new` rejects a zero height, and
/// what comes back in its place is half the surface, *centered* - the pane would jump to the
/// middle of the screen rather than simply being small.
#[test]
fn a_pane_never_collapses_to_nothing_on_a_tiny_screen() {
    for rows in 6..=14 {
        let (chat, agents) = home_geometry(&BTreeMap::new(), rows, 80, Shown::BOTH)
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
// The minimum size, which is a guarantee rather than a request
// ---------------------------------------------------------------------------
//
// Every surface gets a panel hocus can draw a conversation in: the padding gives way first, then
// the configured size, and what is left is the whole of the screen. Below the minimum the panel is
// as big as the screen allows and `ui::render` drops chrome to fit - what must never happen is a
// pane off the edge of the screen, a pane of nothing, or a pane the server has to clip back.

/// Both rectangles on one surface, as `(x, y, width, height)`.
fn both(rows: usize, cols: usize, shown: Shown) -> [(usize, usize, usize, usize); 2] {
    let (chat, agents) = home_geometry(&BTreeMap::new(), rows, cols, shown)
        .unwrap_or_else(|| panic!("some geometry at {rows}x{cols}"));
    [rect(&chat), rect(&agents)]
}

#[test]
fn every_surface_gets_a_pane_that_meets_the_minimum() {
    for rows in 1..=40 {
        for cols in 1..=120 {
            for shown in [Shown::BOTH, only_chat(), only_agents()] {
                // what one half may have of this screen, which is all the minimum can ask for
                let budget = match shown {
                    Shown::BOTH => cols / 2,
                    _ => cols,
                }
                .max(1);
                let (min_w, min_h) = (MIN_PANE_COLS.min(budget), MIN_PANE_ROWS.min(rows));
                let [chat, agents] = both(rows, cols, shown);
                let drawn = [(shown.chat, chat), (shown.agents, agents)];
                for (x, y, w, h) in drawn.into_iter().filter(|(on, _)| *on).map(|(_, r)| r) {
                    let at = format!("{rows}x{cols} {shown:?}: {x},{y} {w}x{h}");
                    assert!(w > 0 && h > 0, "a pane of nothing at {at}");
                    assert!(x + w <= cols, "off the right-hand edge at {at}");
                    assert!(y + h <= rows, "off the bottom at {at}");
                    assert!(w >= min_w, "narrower than {min_w} at {at}");
                    assert!(h >= min_h, "shorter than {min_h} at {at}");
                }
            }
        }
    }
}

/// The two halves are one panel at every size: the conversation starts on the column the list
/// ends on. A single column cannot hold two of anything, which is the one surface where they are
/// allowed to share.
#[test]
fn the_two_halves_always_meet() {
    for rows in 1..=40 {
        for cols in 2..=120 {
            let [(chat_x, _, _, _), (agents_x, _, agents_w, _)] = both(rows, cols, Shown::BOTH);
            assert_eq!(
                agents_x + agents_w,
                chat_x,
                "the halves are apart at {rows}x{cols}: {agents_x}+{agents_w} against {chat_x}"
            );
        }
    }
}

/// Which of the three gives way first, stated as the test: the padding is an inset, and an inset
/// is worth less than a panel anybody can read.
#[test]
fn the_pad_shrinks_before_the_panel_does() {
    let [(chat_x, _, chat_w, _), (x, _, agents_w, _)] = both(24, 56, Shown::BOTH);
    assert!(
        x < DEFAULT_PAD,
        "the configured padding does not fit two halves across 56 columns, so it gave way: {x}"
    );
    assert!(
        chat_w >= MIN_PANE_COLS && agents_w >= MIN_PANE_COLS,
        "{chat_w} {agents_w}"
    );
    assert!(chat_x + chat_w <= 56, "and the panel is still on screen");

    assert_eq!(
        both(50, 200, Shown::BOTH)[1].0,
        DEFAULT_PAD,
        "a screen with room for both keeps the padding it was given"
    );
}

/// A configured size under the minimum is raised to it: a `width` under two halves' worth with
/// both on screen, one half's worth alone, and a `height` under 8 rows, because three rows of
/// pane is one row of conversation and nothing can be said in it.
#[test]
fn a_configured_size_below_the_minimum_is_raised() {
    let (chat, agents) = beside(&config(&[("height", "2"), ("width", "5")]), 50, 200);
    let (_, _, w, h) = panel_of(&chat, &agents);
    assert_eq!((w, h), (2 * MIN_PANE_COLS, MIN_PANE_ROWS));
    assert_eq!(rect(&agents).2, MIN_PANE_COLS);

    let (chat, _) =
        home_geometry(&config(&[("width", "5")]), 50, 200, only_chat()).expect("some geometry");
    assert_eq!(
        rect(&chat).2,
        MIN_PANE_COLS,
        "alone, a half needs only its own floor"
    );
}

/// The two placements must never disagree about the size, at any surface - it is the same
/// arithmetic, and this is what says so.
#[test]
fn the_clear_placement_is_the_same_size_as_the_corner_one() {
    for (rows, cols) in [
        (50, 200),
        (24, 80),
        (24, 60),
        (24, 50),
        (10, 80),
        (8, 80),
        (6, 40),
    ] {
        for shown in [Shown::BOTH, only_chat(), only_agents()] {
            let (chat, agents) = home_geometry(&BTreeMap::new(), rows, cols, shown)
                .unwrap_or_else(|| panic!("some geometry at {rows}x{cols}"));
            let (_, _, w, h) = panel_of(&chat, &agents);
            for alone in [true, false] {
                for step in 0..4 {
                    let clear = panel_placement(
                        &BTreeMap::new(),
                        rows,
                        cols,
                        shown,
                        Some(at(0, 0, cols, rows / 2)),
                        alone,
                        step,
                    )
                    .expect("some geometry");
                    assert_eq!(
                        (clear.cols, clear.rows),
                        (w, h),
                        "{rows}x{cols} {shown:?} alone={alone} step={step}"
                    );
                    assert!(clear.x + clear.cols <= cols && clear.y + clear.rows <= rows);
                }
            }
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
        own_is_floating: Some(true),
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
        state.client = ClientId::new(1);
        state.on_tabs(tabs(0));
        state.client = ClientId::new(2);
        state.on_tabs(tabs(1));
    }

    assert_eq!(
        state.own_tab_index,
        Some(0),
        "the column stayed on the tab it started on"
    );
    assert_eq!(
        state.client_tabs,
        BTreeMap::from([(ClientId::new(1), 0), (ClientId::new(2), 1)]),
        "and both clients were recorded where they actually are"
    );
}

/// Both halves on one tab is the ordinary case, and it must cost nothing: a move here would
/// panic, so this asserts none is attempted as well as that the list's tab is learned.
#[test]
fn the_agent_lists_tab_is_tracked_beside_our_own() {
    let mut state = State {
        own_is_floating: Some(true),
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
        own_is_floating: Some(true),
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
        own_is_floating: Some(true),
        own_plugin_id: 7,
        own_tab_index: Some(3),
        hidden: true,
        hide_pending: 0,
        client: ClientId::new(1),
        client_tabs: BTreeMap::from([(ClientId::new(1), 0), (ClientId::new(2), 3)]),
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
// The two panes hide and show as one
// ---------------------------------------------------------------------------

/// A list on screen beside a suppressed conversation is the `a` keybinding half-arrived: it
/// reveals and focuses the half it names and leaves the other where it was. The panel has one
/// visibility, so the conversation joins it - unfocused, since the user asked for the list.
///
/// This is the case the panes used to be *separated* over, and the separation is what has gone.
#[test]
fn a_visible_agent_list_brings_the_conversation_back_with_it() {
    let mut state = State {
        own_is_floating: Some(true),
        own_plugin_id: 7,
        agents_plugin: Some(9),
        own_tab_index: Some(3),
        hidden: true,
        hide_pending: 0,
        ..Default::default()
    };

    state.on_panes(manifest(3, vec![pane(7, true), pane(9, false)]));

    assert!(!state.hidden, "the panel is up, so both halves are");
    assert!(state.agents_visible);
    assert_eq!(state.own_tab_index, Some(3));
}

/// And the frame between the two hides landing is not that: one Esc suppresses both halves and
/// they land on their own schedules, so a snapshot showing the list still standing beside an
/// already-suppressed conversation is a dismissal in progress rather than a reveal.
///
/// Without `hide_pending` holding it, that frame would read exactly like the test above and put
/// the panel straight back on the press that dismissed it.
#[test]
fn a_hide_only_half_landed_is_not_read_as_a_reveal() {
    let mut state = State {
        own_is_floating: Some(true),
        own_plugin_id: 7,
        agents_plugin: Some(9),
        own_tab_index: Some(3),
        hidden: true,
        hide_pending: 2,
        ..Default::default()
    };

    state.on_panes(manifest(3, vec![pane(7, true), pane(9, false)]));

    assert!(state.hidden, "still going away");
    assert!(state.hide_pending > 0, "and still waiting to see it land");

    // the list's own hide lands a frame later, and that is the whole panel down
    state.on_panes(manifest(3, vec![pane(7, true), pane(9, true)]));
    assert!(state.hidden);
    assert_eq!(state.hide_pending, 0);
}

/// The same arm read the other way: our own hide landing is a hide, whatever the list is doing.
#[test]
fn our_own_hide_landing_is_not_a_reveal() {
    let mut state = State {
        own_is_floating: Some(true),
        own_plugin_id: 7,
        agents_plugin: Some(9),
        hidden: true,
        hide_pending: 2,
        ..Default::default()
    };

    state.on_panes(manifest(3, vec![pane(7, true), pane(9, true)]));

    assert!(state.hidden, "we are away, and stay away");
    assert!(
        state.hide_pending == 0,
        "the hide we asked for has been seen to land"
    );
}

// ---------------------------------------------------------------------------
// A conversation the agent list launched
// ---------------------------------------------------------------------------

/// The list needs a hocus - it draws rows only hocus computes - and the panel is both halves, so
/// that conversation belongs on screen. What it must not do is go asking for a *second* list: a
/// greeting arriving before our own first `PaneUpdate` is the launch that made us, and nothing
/// else can be that early, so the list it came from is the one we already have.
#[test]
fn a_greeting_that_arrives_before_our_first_pane_update_is_the_list_that_made_us() {
    let mut state = State {
        own_is_floating: Some(true),
        own_plugin_id: 7,
        ..Default::default()
    };
    assert!(state.own_tab_index.is_none(), "no manifest yet");

    state.note_greeting_from_agents(true);
    assert!(
        state.agents_open_requested,
        "a list exists and is on its way to announcing itself"
    );
    assert!(!state.hidden, "and the conversation beside it is on screen");
}

/// And a hocus that was already open simply answers with the strip: the greeting is the list
/// saying hello, not the list having opened us.
#[test]
fn a_greeting_to_a_conversation_that_was_already_open_claims_nothing() {
    let mut state = State {
        own_is_floating: Some(true),
        own_plugin_id: 7,
        own_tab_index: Some(0),
        ..Default::default()
    };

    state.note_greeting_from_agents(true);
    assert!(!state.agents_open_requested);

    // nor does a greeting with no marker on it at all
    let mut state = State {
        own_is_floating: Some(true),
        own_plugin_id: 7,
        ..Default::default()
    };
    state.note_greeting_from_agents(false);
    assert!(!state.agents_open_requested);
}

/// A list already asked for is not asked for again, however many manifests arrive while the
/// launch is in flight. `agents_plugin` stays `None` for all of them - the id only arrives with
/// `AGENT_SELECTION` - so without the claim every one of them would open another list.
#[test]
fn a_list_already_asked_for_is_not_asked_for_twice() {
    let mut state = State {
        own_is_floating: Some(true),
        own_plugin_id: 7,
        ..Default::default()
    };

    // The panel is up with no other half, so the first manifest asks for one.
    state.on_panes(manifest(0, vec![pane(7, false)]));
    assert!(state.agents_open_requested, "asked once");
    assert_eq!(state.agents_launches, 1);
    assert!(!state.hidden);

    // Every manifest until the list announces itself finds the same state, and asks nothing.
    for _ in 0..3 {
        state.on_panes(manifest(0, vec![pane(7, false)]));
    }
    assert!(state.agents_open_requested, "the claim still stands");
    assert_eq!(
        state.agents_launches, 1,
        "and no manifest in between asked for a second list"
    );
}

/// A launch that never announces itself - the plugin failed to load, or its pane was closed
/// before `AGENT_SELECTION` went out - leaves the claim standing with no id to clear it against.
/// Nothing in the plugin can see that, so the user's own gesture is the recovery: an explicit
/// reveal drops the claim and asks again.
#[test]
fn a_list_that_never_arrives_is_asked_for_again_on_the_next_reveal() {
    let mut state = State {
        own_is_floating: Some(true),
        own_plugin_id: 7,
        ..Default::default()
    };

    state.on_panes(manifest(0, vec![pane(7, false)]));
    assert_eq!(state.agents_launches, 1, "asked once, and it never arrives");

    // Esc and `c`, with no manifest in between to clear anything.
    state.hide_panel();
    state.reveal_panel();

    assert_eq!(
        state.agents_launches, 2,
        "the reveal grows the panel's missing half rather than waiting on a launch that is gone"
    );

    // And a reveal on its own does too - a dismissal is not what makes the claim recoverable.
    let mut state = State {
        own_is_floating: Some(true),
        own_plugin_id: 7,
        ..Default::default()
    };
    state.on_panes(manifest(0, vec![pane(7, false)]));
    state.hidden = true;
    state.reveal_panel();
    assert_eq!(state.agents_launches, 2);
}

// ---- which rectangle the panes are placed against ---------------------------------------------
//
// An overlay's coordinates are resolved by the server against the whole session surface, which a
// tab occupies in full - so `display_area_*` is the pair the arithmetic has to use.

/// A tab over the whole session surface.
fn a_tab() -> TabInfo {
    TabInfo {
        position: 0,
        tab_id: 0,
        active: true,
        display_area_rows: 50,
        display_area_columns: 200,
        ..TabInfo::default()
    }
}

#[test]
fn the_panes_are_placed_against_the_tab_s_display_area() {
    let tab = a_tab();
    assert_eq!(surface_of(&tab), (50, 200));

    let (rows, cols) = surface_of(&tab);
    let (chat, _) = beside(&BTreeMap::new(), rows, cols);
    let (_, chat_y, chat_w, chat_h) = rect(&chat);
    assert_eq!(
        chat_y + chat_h,
        50 - DEFAULT_PAD,
        "the bottom edge is one padding up from the bottom of the surface"
    );
    assert_eq!(chat_w, 60, "and the width is 30% of the surface");
}

#[test]
fn the_surface_is_what_a_tab_update_caches() {
    let mut state = State {
        own_is_floating: Some(true),
        own_plugin_id: 7,
        ..Default::default()
    };
    state.on_tabs(vec![a_tab()]);
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

// ---------------------------------------------------------------------------------------------
// Ctrl-o: the conversation jumps over a different tiled pane.
//
// `pick_anchor` and `anchored_geometry` are pure functions of the manifest and are tested as
// such; the state-level tests below pin what a press does to the placement over time.

/// A tiled pane as the manifest reports one: selectable, on the tiled layer, with a rect.
fn tiled(id: u32, x: usize, y: usize, cols: usize, rows: usize, focused: bool) -> PaneInfo {
    PaneInfo {
        id,
        is_plugin: false,
        is_focused: focused,
        is_selectable: true,
        pane_x: x,
        pane_y: y,
        pane_columns: cols,
        pane_rows: rows,
        ..Default::default()
    }
}

/// Our own overlay entry, with the rect the server drew us at.
fn overlay_pane(id: u32, x: usize, y: usize, cols: usize, rows: usize) -> PaneInfo {
    PaneInfo {
        is_floating: true,
        ..tiled(id, x, y, cols, rows, false)
    }
    .with_plugin()
}

trait WithPlugin {
    fn with_plugin(self) -> Self;
}
impl WithPlugin for PaneInfo {
    fn with_plugin(mut self) -> Self {
        self.is_plugin = true;
        self
    }
}

fn at(x: usize, y: usize, cols: usize, rows: usize) -> Rect {
    Rect { x, y, cols, rows }
}

#[test]
fn pick_anchor_skips_the_focused_pane() {
    // the focused pane is the biggest by far, and still not a candidate
    let panes = vec![
        tiled(1, 0, 0, 150, 50, true),
        tiled(2, 150, 0, 50, 50, false),
    ];
    assert_eq!(
        pick_anchor(at(3, 14, 60, 33), &panes, &[]),
        Some(PaneId::Terminal(zellij_tile::prelude::TerminalId(2)))
    );
}

#[test]
fn pick_anchor_prefers_the_pane_hocus_is_covering() {
    // pane 2 is untouched, but pane 1 is the one the conversation is sitting on top of - and
    // getting off it is the whole of what the key is for
    let panes = vec![
        tiled(1, 0, 0, 100, 50, false),
        tiled(2, 100, 0, 100, 25, false),
        tiled(3, 100, 25, 100, 25, true),
    ];
    assert_eq!(
        pick_anchor(at(3, 14, 60, 33), &panes, &[]),
        Some(PaneId::Terminal(zellij_tile::prelude::TerminalId(1)))
    );
}

#[test]
fn pick_anchor_breaks_ties_by_the_larger_pane() {
    // nothing overlaps: the conversation is over the focused pane
    let panes = vec![
        tiled(1, 0, 0, 100, 50, true),
        tiled(2, 100, 0, 100, 20, false),
        tiled(3, 100, 20, 100, 30, false),
    ];
    assert_eq!(
        pick_anchor(at(3, 14, 60, 33), &panes, &[]),
        Some(PaneId::Terminal(zellij_tile::prelude::TerminalId(3)))
    );
}

#[test]
fn pick_anchor_ignores_floating_suppressed_and_unselectable_panes() {
    let floating = PaneInfo {
        is_floating: true,
        ..tiled(2, 100, 0, 100, 50, false)
    };
    let suppressed = PaneInfo {
        is_suppressed: true,
        ..tiled(3, 100, 0, 100, 50, false)
    };
    let chrome = PaneInfo {
        is_selectable: false,
        ..tiled(4, 0, 49, 200, 1, false)
    };
    let panes = vec![
        tiled(1, 0, 0, 100, 50, true),
        floating,
        suppressed,
        chrome,
        tiled(5, 100, 0, 100, 10, false),
    ];
    assert_eq!(
        pick_anchor(at(3, 14, 60, 33), &panes, &[]),
        Some(PaneId::Terminal(zellij_tile::prelude::TerminalId(5)))
    );
}

/// The tour is what makes a repeat press walk: a pane already visited is out of the running even
/// when the ranking still says it is the best place to be.
#[test]
fn pick_anchor_skips_panes_already_visited() {
    let panes = vec![
        tiled(1, 0, 0, 100, 25, false),
        tiled(2, 0, 25, 100, 25, false),
        tiled(3, 100, 0, 100, 50, true),
    ];
    assert_eq!(
        pick_anchor(at(3, 14, 60, 33), &panes, &[]),
        Some(PaneId::Terminal(zellij_tile::prelude::TerminalId(2))),
        "the pane the conversation is covering most"
    );
    assert_eq!(
        pick_anchor(
            at(3, 14, 60, 33),
            &panes,
            &[PaneId::Terminal(zellij_tile::prelude::TerminalId(2))]
        ),
        Some(PaneId::Terminal(zellij_tile::prelude::TerminalId(1))),
        "and then the next one, rather than the same one again"
    );
    assert_eq!(
        pick_anchor(
            at(3, 14, 60, 33),
            &panes,
            &[
                PaneId::Terminal(zellij_tile::prelude::TerminalId(2)),
                PaneId::Terminal(zellij_tile::prelude::TerminalId(1))
            ]
        ),
        None,
        "every candidate toured: there is no pane left to be sent clear of"
    );
}

#[test]
fn pick_anchor_falls_back_to_the_only_tiled_pane() {
    // focused, and still the answer: it is the only thing on the tab to be clear of, and the
    // overlay beside it is floating and so is not a pane the layout is made of
    let panes = vec![
        tiled(1, 0, 0, 200, 50, true),
        overlay_pane(7, 3, 14, 60, 33),
    ];
    assert_eq!(
        pick_anchor(at(3, 14, 60, 33), &panes, &[]),
        Some(PaneId::Terminal(zellij_tile::prelude::TerminalId(1)))
    );
    assert_eq!(
        pick_anchor(
            at(3, 14, 60, 33),
            &panes,
            &[PaneId::Terminal(zellij_tile::prelude::TerminalId(1))]
        ),
        None,
        "and nowhere left once it has been toured - the press walks the corners instead"
    );
    assert_eq!(pick_anchor(at(3, 14, 60, 33), &[], &[]), None);
}

#[test]
fn sole_tiled_pane_counts_only_the_layout() {
    assert!(sole_tiled_pane(&[
        tiled(1, 0, 0, 200, 50, true),
        overlay_pane(7, 3, 14, 60, 33),
        PaneInfo {
            is_selectable: false,
            ..tiled(4, 0, 49, 200, 1, false)
        },
    ]));
    assert!(!sole_tiled_pane(&two_panes()));
    assert!(!sole_tiled_pane(&[]));
}

/// The clear placement on the standard 50x200 surface under `state_config`, with the conversation
/// alone on screen: the first corner of the order the anchor implies.
fn clear_of(anchor: Rect, alone: bool) -> (usize, usize, usize, usize) {
    let placed = panel_placement(
        &state_config(),
        50,
        200,
        only_chat(),
        Some(anchor),
        alone,
        0,
    )
    .expect("some geometry");
    (placed.x, placed.y, placed.cols, placed.rows)
}

/// The corner placement's `(width, height)` under `state_config`, which the clear placement must
/// never depart from.
fn corner_size() -> (usize, usize) {
    let (corner, _) = home_geometry(&state_config(), 50, 200, only_chat()).expect("some geometry");
    let (_, _, w, h) = rect(&corner);
    (w, h)
}

#[test]
fn the_clear_placement_goes_right_of_a_pane_on_the_left() {
    let (w, h) = corner_size();
    assert_eq!(
        clear_of(at(0, 0, 100, 50), false),
        (200 - DEFAULT_PAD - w, 50 - DEFAULT_PAD - h, w, h),
        "the right-hand slot, still along the bottom"
    );
}

#[test]
fn the_clear_placement_goes_left_of_a_pane_on_the_right() {
    let (w, h) = corner_size();
    assert_eq!(
        clear_of(at(100, 0, 100, 50), false),
        (DEFAULT_PAD, 50 - DEFAULT_PAD - h, w, h),
        "the default corner, which is already clear of that pane"
    );
}

#[test]
fn the_clear_placement_goes_top_right_when_the_pane_is_the_only_one() {
    let (w, h) = corner_size();
    assert_eq!(
        clear_of(at(0, 0, 200, 50), true),
        (200 - DEFAULT_PAD - w, DEFAULT_PAD, w, h),
        "there is no other side of the screen, so the far corner from the default"
    );
}

#[test]
fn the_clear_placement_picks_the_vertical_slot_that_hides_the_least() {
    let (w, h) = corner_size();
    // a full-width band, so neither horizontal slot helps and only the row is left to choose
    let (_, low_band_y, _, _) = clear_of(at(0, 30, 200, 20), false);
    assert_eq!(
        low_band_y, DEFAULT_PAD,
        "a band along the bottom pushes it up"
    );
    let (_, high_band_y, _, _) = clear_of(at(0, 0, 200, 20), false);
    assert_eq!(
        high_band_y,
        50 - DEFAULT_PAD - h,
        "and a band along the top leaves it where it was"
    );
    assert_eq!(
        clear_of(at(0, 30, 200, 20), false).2,
        w,
        "same size either way"
    );
}

#[test]
fn the_clear_placement_still_chooses_a_row_when_the_column_was_forced() {
    let (w, h) = corner_size();
    // centred left, so the conversation is sent right - and wide enough that the right-hand slot
    // is still on top of it, which leaves the row to make up the difference
    assert_eq!(
        clear_of(at(0, 30, 170, 20), false),
        (200 - DEFAULT_PAD - w, DEFAULT_PAD, w, h),
        "as far off it as the forced column allows"
    );
}

#[test]
fn the_clear_placement_leaves_a_centred_pane_in_the_default_corner() {
    let (w, h) = corner_size();
    assert_eq!(
        clear_of(at(0, 0, 200, 50), false),
        (DEFAULT_PAD, 50 - DEFAULT_PAD - h, w, h),
        "every slot ties against a pane that fills the screen, and ties go to the corner"
    );
}

#[test]
fn the_clear_placement_is_never_clamped_to_the_pane() {
    let (w, h) = corner_size();
    assert_eq!(
        clear_of(at(0, 46, 10, 4), false),
        (200 - DEFAULT_PAD - w, 50 - DEFAULT_PAD - h, w, h),
        "a tiny pane moves the conversation, it does not shrink it to fit"
    );
}

#[test]
fn the_clear_placement_has_nothing_to_say_without_a_surface() {
    assert_eq!(
        panel_placement(
            &BTreeMap::new(),
            0,
            200,
            only_chat(),
            Some(at(0, 0, 100, 50)),
            false,
            0
        ),
        None
    );
    let (w, h) = corner_size();
    assert_eq!(
        clear_of(at(0, 0, 0, 6), false),
        (DEFAULT_PAD, 50 - DEFAULT_PAD - h, w, h),
        "a pane with no size is no anchor at all, and the corner is where that leaves us"
    );
}

/// The refactor's regression net: the corner the cycle starts at is the one the placement has
/// always chosen, for every fixture above.
#[test]
fn the_first_corner_of_the_order_is_where_the_placement_goes() {
    let pad = DEFAULT_PAD;
    let size = corner_size();
    for (anchor, alone) in [
        (at(0, 0, 100, 50), false),
        (at(100, 0, 100, 50), false),
        (at(0, 0, 200, 50), true),
        (at(0, 30, 200, 20), false),
        (at(0, 0, 200, 20), false),
        (at(0, 30, 170, 20), false),
        (at(0, 0, 200, 50), false),
        (at(0, 46, 10, 4), false),
    ] {
        let order = corner_order(Some(anchor), alone, 50, 200, pad, size);
        let first = corner_rect(order[0], 50, 200, pad, size);
        assert_eq!(
            (first.x, first.y, first.cols, first.rows),
            clear_of(anchor, alone),
            "{anchor:?} alone={alone}"
        );
    }
}

/// A hocus over a two-pane tab: the focused pane on the left, where the corner is, and another
/// on the right.
fn two_pane_state() -> State {
    let mut state = placed_state();
    state.on_panes(manifest(0, two_panes()));
    state.last_placement = None;
    state
}

fn two_panes() -> Vec<PaneInfo> {
    vec![
        overlay_pane(7, 3, 14, 60, 33),
        tiled(1, 0, 0, 100, 50, true),
        tiled(2, 100, 0, 100, 50, false),
    ]
}

fn ctrl_o() -> KeyWithModifier {
    KeyWithModifier::new(BareKey::Char('o')).with_ctrl_modifier()
}

/// The conversation's rect from the last placement.
fn placed_chat(state: &State) -> (usize, usize, usize, usize) {
    let (placements, _) = state.last_placement.as_ref().expect("a placement");
    let (_, chat) = placements
        .iter()
        .find(|(id, _)| *id == PaneId::Plugin(zellij_tile::prelude::PluginId(7)))
        .expect("our own pane");
    rect(chat)
}

#[test]
fn ctrl_o_moves_the_conversation_clear_of_the_largest_unfocused_pane() {
    let mut state = two_pane_state();

    assert!(!state.on_key(ctrl_o()));

    assert_eq!(
        state.panel.anchor,
        Some(PaneId::Terminal(zellij_tile::prelude::TerminalId(2)))
    );
    let (_, is_the_users) = state.last_placement.as_ref().expect("a placement");
    assert!(
        is_the_users,
        "sent as the user's own placement, over any drag"
    );
    let (corner_w, corner_h) = corner_size();
    let (x, y, w, h) = placed_chat(&state);
    assert!(
        x + w <= 100,
        "clear of pane 2, which starts at 100: {x} {w}"
    );
    // The best corner for pane 2 is the bottom-left one, which is where the conversation already
    // was - so the press takes the next corner of that pane's ordering rather than restating
    // where it is. A press moves; see `State::reposition`.
    assert_eq!(
        (x, y, w, h),
        (DEFAULT_PAD, DEFAULT_PAD, corner_w, corner_h),
        "the left-hand column pane 2 leaves free, one corner along"
    );
}

#[test]
fn ctrl_o_walks_off_whatever_the_conversation_is_covering() {
    let mut state = placed_state();
    // the user's pane along the top, and two below it: one under the conversation's corner, one
    // on the right
    let panes = vec![
        overlay_pane(7, 3, 14, 60, 33),
        tiled(1, 0, 0, 200, 12, true),
        tiled(2, 0, 12, 100, 38, false),
        tiled(3, 100, 12, 100, 38, false),
    ];
    state.on_panes(manifest(0, panes.clone()));

    state.on_key(ctrl_o());
    assert_eq!(
        state.panel.anchor,
        Some(PaneId::Terminal(zellij_tile::prelude::TerminalId(2))),
        "the pane it is on"
    );
    let (x, _, w, _) = placed_chat(&state);
    assert!(x >= 100, "moved off pane 2, onto pane 3: {x} {w}");

    // the manifest catches up with where the server drew us
    state.on_panes(manifest(
        0,
        std::iter::once(overlay_pane(7, x, 14, w, 33))
            .chain(panes.into_iter().skip(1))
            .collect(),
    ));

    state.on_key(ctrl_o());
    assert_eq!(
        state.panel.anchor,
        Some(PaneId::Terminal(zellij_tile::prelude::TerminalId(3))),
        "now it is on pane 3"
    );
    let (x, _, w, _) = placed_chat(&state);
    assert!(x + w <= 100, "and back off it again: {x} {w}");
}

#[test]
fn ctrl_o_with_one_tiled_pane_moves_to_the_top_right() {
    let mut state = placed_state();
    state.on_panes(manifest(
        0,
        vec![
            overlay_pane(7, 3, 14, 60, 33),
            tiled(1, 0, 0, 200, 50, true),
        ],
    ));
    state.last_placement = None;

    state.on_key(ctrl_o());

    assert_eq!(
        state.panel.anchor,
        Some(PaneId::Terminal(zellij_tile::prelude::TerminalId(1))),
        "the focused pane is admitted when it is the only one"
    );
    let (w, h) = corner_size();
    assert_eq!(
        placed_chat(&state),
        (200 - DEFAULT_PAD - w, DEFAULT_PAD, w, h)
    );
}

#[test]
fn ctrl_o_keeps_the_conversations_current_size() {
    let mut state = placed_state();
    state.on_panes(manifest(
        0,
        vec![
            overlay_pane(7, 3, 27, 80, 20),
            tiled(1, 0, 0, 200, 50, true),
        ],
    ));
    state.last_placement = None;

    state.on_key(ctrl_o());

    let (x, y, width, height) = placed_chat(&state);
    assert_eq!(
        (width, height),
        (80, 20),
        "Ctrl-o moves the current rectangle instead of restoring the configured size"
    );
    assert!(x + width <= 200 && y + height <= 50);
}

/// The bug this whole cycle exists for: two tiled panes with the other one focused is the
/// commonest layout there is, and it offers exactly one candidate - so every press after the first
/// used to re-pick that same pane and do nothing at all.
#[test]
fn ctrl_o_twice_on_the_same_pane_moves_to_the_next_corner() {
    let mut state = two_pane_state();
    state.on_key(ctrl_o());
    let first = placed_chat(&state);
    state.last_placement = None;

    state.on_key(ctrl_o());

    assert_eq!(
        state.panel.anchor,
        Some(PaneId::Terminal(zellij_tile::prelude::TerminalId(2))),
        "the same anchor"
    );
    let second = placed_chat(&state);
    assert_ne!(second, first, "and a different place to be");
    assert_eq!(
        (second.2, second.3),
        (first.2, first.3),
        "the same size - a press moves the conversation, it does not resize it"
    );
    assert!(
        second.0 + second.2 <= 100 || second.1 + second.3 <= 50,
        "still on the screen: {second:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// The cycle: every press moves the conversation somewhere else.

/// Drive `presses` Ctrl-o presses without feeding a fresh manifest between them - which is what
/// the user does, and what the ranking alone cannot answer - and collect where each one landed.
fn walk(state: &mut State, presses: usize) -> Vec<(usize, usize, usize, usize)> {
    (0..presses)
        .map(|press| {
            state.last_placement = None;
            state.on_key(ctrl_o());
            state
                .last_placement
                .as_ref()
                .unwrap_or_else(|| panic!("press {press} placed nothing"));
            placed_chat(state)
        })
        .collect()
}

/// The invariant, stated as the test: on every layout, every press lands somewhere new, on the
/// screen, at the size the corner placement gives.
#[test]
fn ctrl_o_always_moves_the_conversation() {
    let one = vec![
        overlay_pane(7, 3, 14, 60, 33),
        tiled(1, 0, 0, 200, 50, true),
    ];
    let four = vec![
        overlay_pane(7, 3, 14, 60, 33),
        tiled(1, 0, 0, 100, 25, true),
        tiled(2, 100, 0, 100, 25, false),
        tiled(3, 0, 25, 100, 25, false),
        tiled(4, 100, 25, 100, 25, false),
    ];
    let (corner_w, corner_h) = corner_size();
    for (name, panes) in [
        ("one pane", one),
        ("two panes", two_panes()),
        ("four panes", four),
    ] {
        let mut state = placed_state();
        state.on_panes(manifest(0, panes));

        let walked = walk(&mut state, 8);

        for pair in walked.windows(2) {
            assert_ne!(pair[0], pair[1], "a press that went nowhere, on {name}");
        }
        for (x, y, w, h) in walked {
            assert_eq!((w, h), (corner_w, corner_h), "resized on {name}");
            assert!(x + w <= 200 && y + h <= 50, "off screen on {name}: {x} {y}");
        }
    }
}

/// Panes first, corners second: the walk is still "show me the next pane" for as long as there is
/// a pane it has not shown.
#[test]
fn ctrl_o_tours_every_pane_before_it_cycles_corners() {
    let mut state = placed_state();
    state.on_panes(manifest(
        0,
        vec![
            overlay_pane(7, 3, 14, 60, 33),
            tiled(1, 0, 0, 100, 25, true),
            tiled(2, 100, 0, 100, 25, false),
            tiled(3, 0, 25, 100, 25, false),
            tiled(4, 100, 25, 100, 25, false),
        ],
    ));

    let mut anchored = Vec::new();
    for _ in 0..3 {
        state.on_key(ctrl_o());
        anchored.push(state.panel.anchor.expect("an anchor"));
    }

    assert_eq!(anchored.len(), 3);
    let mut distinct = anchored.clone();
    distinct.sort_by_key(|id| format!("{id:?}"));
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        3,
        "three presses, three panes: {anchored:?}"
    );
    assert!(
        !anchored.contains(&PaneId::Terminal(zellij_tile::prelude::TerminalId(1))),
        "and never the pane the user is working in: {anchored:?}"
    );
}

/// One tiled pane: a single destination used to be the whole of the story, so the second press had
/// nowhere to go. Now it walks the four corners and comes back round.
#[test]
fn ctrl_o_cycles_the_corners_when_there_is_one_pane() {
    let mut state = placed_state();
    state.on_panes(manifest(
        0,
        vec![
            overlay_pane(7, 3, 14, 60, 33),
            tiled(1, 0, 0, 200, 50, true),
        ],
    ));

    let walked = walk(&mut state, 5);

    let mut corners = walked[..4].to_vec();
    corners.sort();
    corners.dedup();
    assert_eq!(corners.len(), 4, "four presses, four corners: {walked:?}");
    assert_eq!(walked[4], walked[0], "and the fifth is back at the first");
}

/// A tab of nothing but overlays: no candidate, no anchor, and today nothing happened at all.
#[test]
fn ctrl_o_moves_with_no_tiled_panes_at_all() {
    let mut state = placed_state();
    state.on_panes(manifest(0, vec![overlay_pane(7, 3, 14, 60, 33)]));

    let walked = walk(&mut state, 4);

    assert_eq!(state.panel.anchor, None, "there was nothing to anchor to");
    let mut corners = walked.clone();
    corners.sort();
    corners.dedup();
    assert_eq!(corners.len(), 4, "the four plain corners: {walked:?}");
}

/// The one press that is allowed to do nothing: a panel the size of the screen, where all four
/// corners are the same rectangle. One column of surface is the smallest way to say it.
#[test]
fn ctrl_o_does_nothing_when_every_corner_is_the_same_rectangle() {
    let mut state = State {
        own_is_floating: Some(true),
        own_plugin_id: 7,
        ..Default::default()
    };
    state.on_tabs(vec![TabInfo {
        position: 0,
        tab_id: 0,
        active: true,
        display_area_rows: 8,
        display_area_columns: 1,
        ..TabInfo::default()
    }]);
    state.on_panes(manifest(0, vec![overlay_pane(7, 0, 0, 1, 8)]));
    let filling =
        panel_placement(&state.kdl, 8, 1, only_chat(), None, false, 0).expect("a rectangle");
    for step in 0..4 {
        assert_eq!(
            panel_placement(&state.kdl, 8, 1, only_chat(), None, false, step),
            Some(filling),
            "every corner is the whole of the screen"
        );
    }
    state.last_placement = None;

    state.on_key(ctrl_o());

    assert_eq!(
        state.last_placement, None,
        "nowhere to go, so nothing is asked of the server"
    );
}

/// A pane the tour has visited and the user has since closed must not hold its slot: the press
/// that came round to it would find it gone and go nowhere new.
#[test]
fn a_closed_pane_does_not_hold_a_slot_in_the_tour() {
    let mut state = placed_state();
    let panes = vec![
        overlay_pane(7, 3, 14, 60, 33),
        tiled(1, 0, 0, 100, 50, true),
        tiled(2, 100, 0, 50, 50, false),
        tiled(3, 150, 0, 50, 50, false),
    ];
    state.on_panes(manifest(0, panes));

    state.on_key(ctrl_o());
    state.on_key(ctrl_o());
    assert_eq!(
        state.panel.warp_visited.len(),
        2,
        "two panes toured: {:?}",
        state.panel.warp_visited
    );

    // pane 2 closed, and pane 3 took its room
    state.on_panes(manifest(
        0,
        vec![
            overlay_pane(7, 3, 14, 60, 33),
            tiled(1, 0, 0, 100, 50, true),
            tiled(3, 100, 0, 100, 50, false),
        ],
    ));

    assert_eq!(
        state.panel.warp_visited,
        vec![PaneId::Terminal(zellij_tile::prelude::TerminalId(3))],
        "the pane that is gone is out of the tour"
    );
    state.on_key(ctrl_o());
    assert_eq!(
        state.panel.anchor,
        Some(PaneId::Terminal(zellij_tile::prelude::TerminalId(3))),
        "and it is not offered again"
    );
}

/// The whole of the stability guarantee, from the side a user sees: a tab that re-tiles under
/// the conversation leaves it exactly where it was. Following the pane used to be the behaviour,
/// and it is what made an unrelated pane close move the panel across the screen.
#[test]
fn a_re_tile_does_not_move_the_conversation() {
    let mut state = two_pane_state();
    state.on_key(ctrl_o());
    let parked = state.panel.placed_rect.expect("a placement");
    state.last_placement = None;

    // the same manifest again: nothing moved, nothing to say
    state.on_panes(manifest(0, two_panes()));
    assert_eq!(
        state.last_placement, None,
        "an unchanged tab asks for nothing"
    );

    // pane 2 was re-tiled narrower
    state.on_panes(manifest(
        0,
        vec![
            overlay_pane(7, 103, 14, 60, 33),
            tiled(1, 0, 0, 150, 50, true),
            tiled(2, 150, 0, 50, 50, false),
        ],
    ));

    assert_eq!(
        state.last_placement, None,
        "a re-tile is not a request to move anything"
    );
    assert_eq!(state.panel.placed_rect, Some(parked), "and nothing moved");
    assert_eq!(
        state.panel.anchor_rect,
        Some(at(150, 0, 50, 50)),
        "the anchor is still tracked - the next press ranks against it"
    );
}

/// The same rule for the pane the conversation was sent clear of: it going away releases the
/// anchor, and releases nothing else. Snapping back to the corner here was the most visible half
/// of the old behaviour.
#[test]
fn closing_the_anchor_pane_leaves_the_conversation_where_it_is() {
    let mut state = two_pane_state();
    state.on_key(ctrl_o());
    let parked = state.panel.placed_rect.expect("a placement");
    state.last_placement = None;

    state.on_panes(manifest(
        0,
        vec![
            overlay_pane(7, 103, 14, 60, 33),
            tiled(1, 0, 0, 200, 50, true),
        ],
    ));

    assert_eq!((state.panel.anchor, state.panel.anchor_rect), (None, None));
    assert_eq!(state.last_placement, None, "a pane closing places nothing");
    assert_eq!(state.panel.placed_rect, Some(parked), "and moves nothing");
}

/// The corner survives the anchor that chose it, which is what makes the guarantee hold through
/// the one event that genuinely has to re-place: a terminal resize. Without the remembered corner
/// this is where the delayed jump home would happen.
#[test]
fn a_resize_keeps_the_corner_the_last_press_chose() {
    let mut state = two_pane_state();
    state.on_key(ctrl_o());
    let corner = state.panel.placed_corner.expect("a corner");
    assert_ne!(corner, CORNERS[0], "the press left the default corner");

    // the anchor closes, and then the terminal is made smaller
    state.on_panes(manifest(
        0,
        vec![
            overlay_pane(7, 103, 14, 60, 33),
            tiled(1, 0, 0, 200, 50, true),
        ],
    ));
    state.last_placement = None;
    state.on_tabs(vec![TabInfo {
        display_area_rows: 40,
        display_area_columns: 160,
        ..a_tab()
    }]);

    let expected =
        rect_in_corner(&state.kdl, 40, 160, state.shown(), corner, None).expect("a rectangle");
    assert_eq!(state.panel.placed_corner, Some(corner), "the same corner");
    assert_eq!(
        placed_chat(&state),
        (expected.x, expected.y, expected.cols, expected.rows),
        "resolved against the new screen, not dragged home"
    );
}

/// The list is the other half of "both panels stay put": it is placed from the same call, so a
/// manifest that placed nothing cannot have moved it either.
#[test]
fn a_pane_closing_does_not_move_the_agent_list() {
    let mut state = two_pane_state();
    state.pipe(selection(9));
    state.on_panes(manifest(0, list_beside()));
    // The press is what gives `track_anchor` an anchor to lose: with none, it returns at its
    // first guard and the assertion below holds however the placement arms are written.
    state.on_key(ctrl_o());
    state.last_placement = None;

    state.on_panes(manifest(
        0,
        vec![
            overlay_pane(9, 3, 14, 30, 33),
            overlay_pane(7, 33, 14, 30, 33),
            tiled(1, 0, 0, 200, 50, true),
        ],
    ));

    assert_eq!(
        state.last_placement, None,
        "neither half is placed by a pane closing"
    );
}

/// `follow_tab` releases the anchor before its own placement (a host call with a reply, so not
/// driven here); the manifest of the tab we landed on is the other route, and the one every
/// cross-tab move ends in.
///
/// Nothing is placed on arrival, and nothing needs to be: the move carries the overlay's exact
/// coordinates and its `OverlayCarry` across (`Tab::overlay_placement_for`), and every tab
/// resolves against the same session surface.
#[test]
fn arriving_on_another_tab_drops_the_anchor() {
    let mut state = two_pane_state();
    state.on_key(ctrl_o());
    let parked = state.panel.placed_rect.expect("a placement");
    state.last_placement = None;

    state.on_panes(manifest(
        1,
        vec![
            overlay_pane(7, 103, 14, 60, 33),
            tiled(3, 0, 0, 200, 50, true),
        ],
    ));

    assert_eq!(state.own_tab_index, Some(1));
    assert_eq!((state.panel.anchor, state.panel.anchor_rect), (None, None));
    assert_eq!(state.last_placement, None, "the move carried the geometry");
    assert_eq!(state.panel.placed_rect, Some(parked));
}

#[test]
fn a_hidden_panel_keeps_its_anchor() {
    let mut state = two_pane_state();
    state.on_key(ctrl_o());
    let anchored = placed_chat(&state);

    state.hide_panel();
    assert_eq!(
        state.panel.anchor,
        Some(PaneId::Terminal(zellij_tile::prelude::TerminalId(2)))
    );
    state.reveal_panel();

    assert_eq!(
        state.panel.anchor,
        Some(PaneId::Terminal(zellij_tile::prelude::TerminalId(2)))
    );
    assert_eq!(placed_chat(&state), anchored, "revealed where it was");
}

// ---------------------------------------------------------------------------------------------
// The agent list is the other half of the panel: it is placed with the conversation, cut at the
// split, and a press in either half moves the whole thing.

/// The manifest with the list open beside the conversation, both halves at home under
/// `state_config`: the list on the left, the conversation from the divider to the right.
fn list_beside() -> Vec<PaneInfo> {
    vec![
        overlay_pane(9, 3, 14, 30, 33),
        overlay_pane(7, 33, 14, 30, 33),
        tiled(1, 0, 0, 100, 50, true),
        tiled(2, 100, 0, 100, 50, false),
    ]
}

/// A hocus over the two-pane tab with an agent list open beside it, the panel at home.
fn agents_beside_state() -> State {
    let mut state = two_pane_state();
    state.pipe(selection(9));
    state.on_panes(manifest(0, list_beside()));
    state.last_placement = None;
    state
}

/// Esc in the agent list, as `hocus-agents` sends it: that pane hides itself and asks hocus for
/// the other half, being the only one of the two that knows both plugin ids.
fn list_hide() -> PipeMessage {
    PipeMessage {
        source: PipeSource::Plugin(9),
        name: hocus_proto::wire::AGENT_STRIP_HIDE.to_owned(),
        payload: None,
        args: BTreeMap::new(),
        is_private: true,
        pane_id: None,
    }
}

/// Ctrl-o in the agent list, as `hocus-agents` sends it: the pane has no geometry of its own, so
/// the key is a request rather than a move.
fn list_reposition() -> PipeMessage {
    PipeMessage {
        source: PipeSource::Plugin(9),
        name: hocus_proto::wire::AGENT_STRIP_REPOSITION.to_owned(),
        payload: None,
        args: BTreeMap::new(),
        is_private: true,
        pane_id: None,
    }
}

/// One half's rect from the last placement, and `None` when that placement did not name it.
fn placed_pane(state: &State, plugin_id: u32) -> Option<(usize, usize, usize, usize)> {
    let (placements, _) = state.last_placement.as_ref()?;
    placements
        .iter()
        .find(|(id, _)| *id == PaneId::Plugin(zellij_tile::prelude::PluginId(plugin_id)))
        .map(|(_, coordinates)| rect(coordinates))
}

/// A list announcing itself is placed as the left half of the panel, and the conversation is
/// cut down to the right half in the same call - one batch, which is what links the two.
#[test]
fn a_list_that_opens_is_placed_as_the_left_half_of_the_panel() {
    let mut state = two_pane_state();
    assert_eq!(
        state.shown(),
        only_chat(),
        "no list yet, so the conversation has the whole panel"
    );

    state.pipe(selection(9));
    assert_eq!(
        state.shown(),
        Shown::BOTH,
        "a list that announces itself is on screen"
    );
    let (placements, _) = state.last_placement.clone().expect("a placement");
    assert_eq!(
        placements.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        vec![
            PaneId::Plugin(zellij_tile::prelude::PluginId(7)),
            PaneId::Plugin(zellij_tile::prelude::PluginId(9))
        ],
        "both halves, in one call"
    );
    assert_eq!(
        placed_pane(&state, 9),
        Some((3, 14, 30, 33)),
        "the list on the left"
    );
    assert_eq!(
        placed_pane(&state, 7),
        Some((33, 14, 30, 33)),
        "the conversation from the divider"
    );
}

/// A half found suppressed on its own is put back, and the cut never moves for it.
///
/// Nothing in hocus hides one half any more, so a manifest that shows one is something outside
/// the panel having suppressed it - and the repair is to bring it back rather than to widen the
/// survivor over it. The rectangles are the evidence: both halves keep the share they had
/// throughout, so the panel the user gets back is the panel they dismissed.
#[test]
fn a_list_suppressed_on_its_own_is_put_back_rather_than_widened_over() {
    let mut state = agents_beside_state();

    state.on_panes(manifest(
        0,
        vec![
            PaneInfo {
                is_suppressed: true,
                ..overlay_pane(9, 3, 14, 30, 33)
            },
            overlay_pane(7, 33, 14, 30, 33),
            tiled(1, 0, 0, 100, 50, true),
            tiled(2, 100, 0, 100, 50, false),
        ],
    ));
    assert!(!state.agents_visible, "the manifest is read");
    assert!(!state.hidden, "and the panel is still up");
    assert_eq!(
        state.last_placement, None,
        "the cut does not depend on what is suppressed, so nothing is placed"
    );

    // and the list coming back changes no rectangle either
    state.on_panes(manifest(0, list_beside()));
    assert!(state.agents_visible);
    assert_eq!(state.last_placement, None);
}

/// What *does* move the cut is a half that stops existing. A list whose pane the user closed
/// leaves the conversation nothing to share with, so it takes the whole panel.
#[test]
fn a_list_that_closes_gives_its_width_to_the_conversation() {
    let mut state = agents_beside_state();

    state.on_panes(manifest(0, two_panes()));

    assert_eq!(state.shown(), only_chat(), "no list to hold a half");
    assert_eq!(
        placed_pane(&state, 7),
        Some((3, 14, 60, 33)),
        "the conversation fills the panel"
    );
    assert_eq!(
        placed_pane(&state, 9),
        None,
        "and a pane that is gone is not placed"
    );
}

/// Hiding the panel places nothing - both halves go together, so no rectangle changes - and the
/// reveal brings it back exactly as it was.
#[test]
fn hiding_the_panel_moves_no_rectangle() {
    let mut state = agents_beside_state();

    state.hide_panel();
    assert!(state.hidden);
    assert!(state.hide_pending > 0, "and waits to see the hide land");
    assert_eq!(
        state.last_placement, None,
        "the cut is the same panel either way"
    );

    state.reveal_panel();
    assert!(!state.hidden);
    assert_eq!(placed_pane(&state, 9), Some((3, 14, 30, 33)));
    assert_eq!(placed_pane(&state, 7), Some((33, 14, 30, 33)));
}

/// A seam drag happens in the server and claims nothing; the manifest is how it reaches the next
/// placement, as the list's share of the two halves' joint width.
#[test]
fn a_dragged_divider_is_read_back_off_the_manifest_and_kept() {
    let mut state = agents_beside_state();

    // the user dragged the divider two columns right
    state.on_panes(manifest(
        0,
        vec![
            overlay_pane(9, 3, 14, 32, 33),
            overlay_pane(7, 35, 14, 28, 33),
            tiled(1, 0, 0, 100, 50, true),
            tiled(2, 100, 0, 100, 50, false),
        ],
    ));
    assert_eq!(state.split, Some((32, 60)));
    assert_eq!(
        state.last_placement, None,
        "reading the split places nothing"
    );

    // a placement for any other reason cuts the panel where the user left the divider - here the
    // panel being dismissed and brought back
    state.hide_panel();
    state.reveal_panel();
    assert_eq!(placed_pane(&state, 9), Some((3, 14, 32, 33)));
    assert_eq!(placed_pane(&state, 7), Some((35, 14, 28, 33)));

    // a wider panel is cut in the same proportion: 32 of 60 is 48 of 90
    state.kdl = config(&[("width", "90")]);
    state.apply_overlay_geometry();
    assert_eq!(placed_pane(&state, 9), Some((3, 14, 48, 33)));
    assert_eq!(placed_pane(&state, 7), Some((51, 14, 42, 33)));
}

/// The manifest the server sends back after hocus's own placement shows the halves side by side
/// too, cut where the split floors to a column. That is not a drag and must not be read as one:
/// a ratio replaced by its floor is a smaller ratio, and the next placement would floor it again.
#[test]
fn a_manifest_echoing_our_own_placement_leaves_the_split_alone() {
    let mut state = agents_beside_state();
    // a third of the panel, which no width of the panel's divides exactly
    state.split = Some((1, 3));
    state.kdl = config(&[("width", "100")]);
    state.apply_overlay_geometry();
    assert_eq!(placed_pane(&state, 9).map(|(_, _, cols, _)| cols), Some(33));

    state.on_panes(manifest(0, echoed_placement(&state)));
    assert_eq!(
        state.split,
        Some((1, 3)),
        "33 of 100 is our own cut of a third"
    );

    // and with nothing read off the manifest, the configured split stays configured
    let mut state = agents_beside_state();
    assert_eq!(state.split, None, "the list at home is the configured half");
    state.on_panes(manifest(0, list_beside()));
    assert_eq!(state.split, None);
}

/// The whole of what the echo guard is for: a terminal resized back and forth keeps the divider
/// where it was, rather than walking it left a column at a time.
#[test]
fn the_split_does_not_drift_across_repeated_resizes() {
    let mut state = agents_beside_state();
    state.split = Some((1, 3));
    for width in ["100", "70", "100", "70", "100", "70", "100"] {
        state.kdl = config(&[("width", width)]);
        state.apply_overlay_geometry();
        state.on_panes(manifest(0, echoed_placement(&state)));
    }
    assert_eq!(state.split, Some((1, 3)));
    assert_eq!(
        placed_pane(&state, 9).map(|(_, _, cols, _)| cols),
        Some(33),
        "a third of 100, as on the first resize"
    );
}

/// The manifest the server would send back for the last placement: both halves drawn exactly
/// where they were asked to go, over the two-pane tab.
fn echoed_placement(state: &State) -> Vec<PaneInfo> {
    let (x, y, cols, rows) = placed_pane(state, 9).expect("the list was placed");
    let list = overlay_pane(9, x, y, cols, rows);
    let (x, y, cols, rows) = placed_pane(state, 7).expect("the conversation was placed");
    let chat = overlay_pane(7, x, y, cols, rows);
    vec![
        list,
        chat,
        tiled(1, 0, 0, 100, 50, true),
        tiled(2, 100, 0, 100, 50, false),
    ]
}

/// The server's floor for a seam drag is lower than hocus's, and the user's divider wins: a
/// split dragged past what a half can draw a conversation in is read back and kept, rather than
/// pulled to the floor on the next placement - which would move a divider the user just placed.
#[test]
fn a_split_dragged_below_the_floor_is_kept() {
    let mut state = agents_beside_state();
    state.on_panes(manifest(
        0,
        vec![
            overlay_pane(9, 3, 14, 50, 33),
            overlay_pane(7, 53, 14, 10, 33),
            tiled(1, 0, 0, 100, 50, true),
            tiled(2, 100, 0, 100, 50, false),
        ],
    ));
    // a ten-column half is well under `MIN_PANE_COLS`
    assert_eq!(state.split, Some((50, 60)), "read as dragged");

    state.apply_overlay_geometry();
    assert_eq!(placed_pane(&state, 9), Some((3, 14, 50, 33)));
    assert_eq!(placed_pane(&state, 7), Some((53, 14, 10, 33)));
}

/// Ctrl-o in the list moves the panel only when the list is here to move: with no list, or with
/// the list on another tab, the request does nothing. A press ranks the corners against the
/// panes of *our* tab, and placing a parted list against those would put it clear of nothing -
/// and mark it placed while it is still elsewhere.
#[test]
fn a_list_that_is_not_here_is_not_repositioned() {
    let mut state = two_pane_state();

    state.pipe(list_reposition());
    assert_eq!(state.last_placement, None, "there is no list to move");

    let mut state = agents_beside_state();
    state.agents_tab_index = Some(1);
    state.pipe(list_reposition());
    assert_eq!(
        state.last_placement, None,
        "the list is on another tab, and our manifest says nothing about the panes there"
    );

    // Ctrl-o in the conversation is not gated: it is on our tab by definition
    state.reposition();
    assert!(state.last_placement.is_some());
}

/// Only two halves side by side say anything about the split: a manifest with one half filling
/// the panel, or the two not yet redrawn beside each other, leaves it alone.
#[test]
fn a_manifest_without_the_halves_side_by_side_leaves_the_split_alone() {
    let mut state = agents_beside_state();
    state.split = Some((20, 60));

    state.on_panes(manifest(
        0,
        vec![
            PaneInfo {
                is_suppressed: true,
                ..overlay_pane(9, 3, 14, 30, 33)
            },
            overlay_pane(7, 3, 14, 60, 33),
            tiled(1, 0, 0, 200, 50, true),
        ],
    ));
    assert_eq!(state.split, Some((20, 60)), "the list is hidden");

    state.on_panes(manifest(
        0,
        vec![
            overlay_pane(9, 3, 14, 30, 33),
            overlay_pane(7, 3, 3, 60, 33),
            tiled(1, 0, 0, 200, 50, true),
        ],
    ));
    assert_eq!(
        state.split,
        Some((20, 60)),
        "the two are not beside each other"
    );
}

/// The list's own Ctrl-o, and the whole of what it is for: it walks the panel off the pane it was
/// covering.
#[test]
fn ctrl_o_in_the_list_moves_the_panel_clear_of_the_largest_unfocused_pane() {
    let mut state = agents_beside_state();

    state.pipe(list_reposition());

    assert_eq!(
        state.panel.anchor,
        Some(PaneId::Terminal(zellij_tile::prelude::TerminalId(2))),
        "the largest pane the user is not working in"
    );
    let (_, is_the_users) = state.last_placement.as_ref().expect("a placement");
    assert!(
        is_the_users,
        "sent as the user's own placement, over any drag"
    );
    let (agents_x, _, agents_w, _) = placed_pane(&state, 9).expect("the list is placed");
    let (chat_x, _, chat_w, _) = placed_pane(&state, 7).expect("and the conversation");
    assert!(
        chat_x + chat_w <= 100 && agents_x + agents_w <= 100,
        "clear of pane 2, which starts at 100: {agents_x} {agents_w} / {chat_x} {chat_w}"
    );
    assert_eq!(chat_x, agents_x + agents_w, "still one panel");
}

/// A press in either half moves the whole panel: both ids in the placement, the split intact.
#[test]
fn a_press_in_either_half_places_the_whole_panel() {
    let mut state = agents_beside_state();

    state.pipe(list_reposition());
    let (placements, _) = state.last_placement.clone().expect("a placement");
    assert_eq!(
        placements.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        vec![
            PaneId::Plugin(zellij_tile::prelude::PluginId(7)),
            PaneId::Plugin(zellij_tile::prelude::PluginId(9))
        ],
        "both halves, from the list's press"
    );
    let from_the_list = (placed_pane(&state, 7), placed_pane(&state, 9));

    let mut state = agents_beside_state();
    state.on_key(ctrl_o());
    let (placements, _) = state.last_placement.clone().expect("a placement");
    assert_eq!(
        placements.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        vec![
            PaneId::Plugin(zellij_tile::prelude::PluginId(7)),
            PaneId::Plugin(zellij_tile::prelude::PluginId(9))
        ],
        "and both halves from the conversation's"
    );
    assert_eq!(
        (placed_pane(&state, 7), placed_pane(&state, 9)),
        from_the_list,
        "to the same place: it is one panel and one tour"
    );
    let (agents_x, _, agents_w, _) = placed_pane(&state, 9).expect("the list");
    assert_eq!(
        placed_pane(&state, 7).map(|(x, ..)| x),
        Some(agents_x + agents_w),
        "cut at the split"
    );
}

/// A press moves, every time - the same guarantee the conversation's has, walked pane to pane and
/// then round the corners.
#[test]
fn ctrl_o_in_the_list_always_moves_it() {
    let mut state = agents_beside_state();
    // Home, as `agents_beside_state` leaves it.
    let mut was = (3, 14, 30, 33);
    let mut corners = std::collections::BTreeSet::new();

    for press in 0..5 {
        state.last_placement = None;
        state.pipe(list_reposition());
        let placed = placed_pane(&state, 9).expect("a placement");
        assert_ne!(placed, was, "press {press} moved the list nowhere");
        corners.insert(placed);
        was = placed;
        // The manifest the server would send back, so the next press ranks against where the
        // panel actually is rather than where it started.
        let (x, y, cols, rows) = placed;
        let (chat_x, chat_y, chat_cols, chat_rows) = placed_pane(&state, 7).expect("a placement");
        state.on_panes(manifest(
            0,
            vec![
                overlay_pane(9, x, y, cols, rows),
                overlay_pane(7, chat_x, chat_y, chat_cols, chat_rows),
                tiled(1, 0, 0, 100, 50, true),
                tiled(2, 100, 0, 100, 50, false),
            ],
        ));
    }
    assert_eq!(
        corners.len(),
        4,
        "the four corners and nothing outside them - the fifth press re-opens the tour"
    );
}

/// A user who resized the panel keeps that size through a move: the panel is read back as the
/// rectangle round both halves.
#[test]
fn ctrl_o_keeps_the_panels_current_size() {
    let mut state = two_pane_state();
    state.pipe(selection(9));
    // the user ctrl-dragged the panel's top edge up, and the divider across
    state.on_panes(manifest(
        0,
        vec![
            overlay_pane(9, 3, 7, 45, 40),
            overlay_pane(7, 48, 7, 35, 40),
            tiled(1, 0, 0, 100, 50, true),
            tiled(2, 100, 0, 100, 50, false),
        ],
    ));
    state.last_placement = None;

    state.pipe(list_reposition());

    let (agents_x, _, agents_w, agents_h) = placed_pane(&state, 9).expect("a placement");
    let (chat_x, _, chat_w, chat_h) = placed_pane(&state, 7).expect("a placement");
    assert_eq!(
        (agents_w + chat_w, agents_h, chat_h),
        (80, 40, 40),
        "the size the manifest reports, not the configured one"
    );
    assert_eq!(chat_x, agents_x + agents_w);
    assert_eq!(
        (agents_w, chat_w),
        (45, 35),
        "and the split the user dragged"
    );
}

// ---------------------------------------------------------------------------
// One panel, one visibility
// ---------------------------------------------------------------------------

/// Esc in the conversation is the panel's dismissal, not this pane's.
#[test]
fn esc_in_the_conversation_puts_the_whole_panel_away() {
    let mut state = agents_beside_state();
    assert!(!state.hidden);

    state.on_key(KeyWithModifier::new(BareKey::Esc));

    assert!(state.hidden);
    assert!(
        state.hide_pending > 0,
        "both hides are out and neither seen yet"
    );
}

/// And Esc in the list arrives here, over the wire, to the same effect: the list has already
/// hidden itself, so what this call is for is the conversation.
#[test]
fn esc_in_the_list_puts_the_whole_panel_away() {
    let mut state = agents_beside_state();

    state.pipe(list_hide());

    assert!(state.hidden);
    assert!(state.hide_pending > 0);
}

/// A dismissed panel asks for nothing. Without this the list `open_agents_pane` had claimed
/// would arrive - unsuppressed, and alone - on top of a panel the user had just put away.
#[test]
fn hiding_the_panel_drops_a_list_it_had_asked_for() {
    let mut state = two_pane_state();
    state.open_agents_pane();
    assert!(state.agents_open_requested);

    state.hide_panel();

    assert!(!state.agents_open_requested);
}

/// The panel that comes back is the whole panel: revealing the conversation with no list beside
/// it asks for one rather than leaving the half open.
#[test]
fn revealing_the_conversation_asks_for_the_missing_half() {
    let mut state = two_pane_state();
    state.hide_panel();
    state.hide_pending = 0;
    assert!(state.agents_plugin.is_none(), "no list to unsuppress");

    state.reveal_panel();

    assert!(!state.hidden);
    assert!(
        state.agents_open_requested,
        "so the panel grows its other half"
    );
}

/// The `a` keybinding's side of the same rule: the list is already up and focused, so this half
/// joins it and asks for nothing - a second list is the one thing this must not open.
#[test]
fn joining_a_revealed_list_opens_no_second_list() {
    let mut state = agents_beside_state();
    state.hide_panel();
    state.hide_pending = 0;

    state.reveal_self_beside_agents();

    assert!(!state.hidden);
    assert!(!state.agents_open_requested);
    assert_eq!(
        placed_pane(&state, 9),
        Some((3, 14, 30, 33)),
        "and the panel comes back at the split"
    );
    assert_eq!(placed_pane(&state, 7), Some((33, 14, 30, 33)));
}

/// A list asked for before the Esc still arrives afterwards - `hide_panel` has no id for it and
/// so cannot hide it - and the moment it announces one is the moment it can be put down with the
/// panel it belongs to. Without this it lands unsuppressed and alone, and the next manifest reads
/// that half as a reveal and puts the whole panel back on the very press that dismissed it.
#[test]
fn a_list_that_arrives_after_the_esc_arrives_hidden() {
    let mut state = two_pane_state();
    state.open_agents_pane();
    state.hide_panel();
    state.visibility_calls.clear();

    state.pipe(selection(9));

    assert!(
        state
            .visibility_calls
            .contains(&(PaneId::Plugin(zellij_tile::prelude::PluginId(9)), false)),
        "the half that could not be named at the Esc is named here"
    );
    assert!(!state.agents_visible, "so the panel is down, both halves");

    // and the frame that follows is a hide landing, not a reveal
    state.on_panes(manifest(0, vec![pane(7, true), pane(9, true)]));
    assert!(state.hidden);
    assert_eq!(state.hide_pending, 0);
}

/// A job that fires with `show: true` reveals the *panel*, and says so: `show_self` on its own
/// left `hidden` true and any pending hide standing, so the manifest that followed was swallowed
/// by the stale-hide guard and hocus sat on screen believing itself dismissed.
#[test]
fn a_job_showing_the_conversation_shows_the_whole_panel() {
    let mut state = agents_beside_state();
    state.hide_panel();
    assert!(state.hide_pending > 0);
    state.visibility_calls.clear();

    state.show_panel();

    assert!(!state.hidden, "the intent is flipped at the source");
    assert!(state.hide_pending == 0, "so the hide it replaces is void");
    assert!(state
        .visibility_calls
        .contains(&(PaneId::Plugin(zellij_tile::prelude::PluginId(7)), true)));
    assert!(
        state
            .visibility_calls
            .contains(&(PaneId::Plugin(zellij_tile::prelude::PluginId(9)), true)),
        "and the list comes back with it"
    );
}

/// Which is what keeps the guard from wedging: the late frame the two hides did produce is read
/// as the disagreement it now is, rather than held against a panel that is up again.
#[test]
fn a_stale_hide_does_not_hold_a_panel_that_came_back() {
    let mut state = agents_beside_state();
    state.hide_panel();
    state.show_panel();
    state.visibility_calls.clear();

    state.on_panes(manifest(0, vec![pane(7, false), pane(9, true)]));

    assert!(
        state
            .visibility_calls
            .contains(&(PaneId::Plugin(zellij_tile::prelude::PluginId(9)), true)),
        "the list is put back beside us rather than the frame being swallowed"
    );
    assert!(!state.hidden);
}

/// And a hide that lands on one half only is restated rather than waited on. This arm is the end
/// of the line for such a frame - nothing else will report it again - so without the restatement
/// `hide_pending` stands forever and every manifest after it is swallowed.
#[test]
fn a_hide_that_landed_on_one_half_is_restated_on_the_other() {
    let mut state = agents_beside_state();
    state.hide_panel();
    state.visibility_calls.clear();

    state.on_panes(manifest(0, vec![pane(7, true), pane(9, false)]));

    assert!(
        state
            .visibility_calls
            .contains(&(PaneId::Plugin(zellij_tile::prelude::PluginId(9)), false)),
        "the half still standing is asked down again"
    );
    assert!(state.hidden, "and the frame is not read as a reveal");
    assert!(state.hide_pending > 0, "the hide is still not seen to land");

    // which it then does, on both halves
    state.on_panes(manifest(0, vec![pane(7, true), pane(9, true)]));
    assert_eq!(state.hide_pending, 0);
}

/// A list the user closed is not a missing half to grow back. `agents_plugin` is cleared on the
/// very manifest that reports the pane gone, so without remembering the closure the repair arm
/// reads "no list" and launches another one - and the list cannot be closed at all.
#[test]
fn closing_the_list_does_not_launch_another_one() {
    let mut state = agents_beside_state();
    let launched = state.agents_launches;

    state.on_panes(manifest(0, two_panes()));
    assert!(state.agents_closed);
    assert_eq!(
        state.agents_launches, launched,
        "the conversation takes the whole panel instead"
    );

    // and stays closed, however many manifests arrive
    for _ in 0..3 {
        state.on_panes(manifest(0, two_panes()));
    }
    assert_eq!(state.agents_launches, launched);

    // until the user asks for a panel again
    state.hide_panel();
    state.reveal_panel();
    assert_eq!(state.agents_launches, launched + 1);
}

// ---- a hocus that is not a panel ------------------------------------------------------------
//
// `pane { plugin location="hocus" }` in a user's layout is a plain tiled pane, and every one of
// the behaviours above belongs to the overlay. Told apart by `is_floating` off the manifest - see
// `State::is_panel`.

/// The pane a layout gives hocus: a plugin pane on the tiled layer, which is the one thing that
/// distinguishes it from the overlay every other fixture here builds.
fn tiled_own(id: u32) -> PaneInfo {
    PaneInfo {
        id,
        is_plugin: true,
        is_selectable: true,
        pane_columns: 80,
        pane_rows: 40,
        ..Default::default()
    }
}

/// A tiled hocus grew the panel's other half. The first `PaneUpdate` took the repair arm - no
/// list, none closed, nothing suppressed - and launched an agent list over a session that had
/// asked for a conversation in its layout and nothing else. The list's own greeting then launched
/// a *second*, overlay hocus, because the greeting names hocus's url with the overlay
/// configuration and `(location, configuration)` is plugin identity, so the bare `location="hocus"`
/// of the layout does not match and launch-or-reuse spawns rather than reuses.
#[test]
fn a_hocus_in_the_layout_grows_no_agent_list() {
    let mut state = State {
        own_plugin_id: 7,
        kdl: state_config(),
        ..Default::default()
    };
    state.on_tabs(vec![TabInfo {
        position: 0,
        active: true,
        display_area_rows: 50,
        display_area_columns: 200,
        ..Default::default()
    }]);

    for _ in 0..4 {
        state.on_panes(manifest(
            0,
            vec![tiled_own(7), tiled(1, 0, 0, 200, 50, true)],
        ));
    }

    assert!(
        !state.is_panel(),
        "the manifest says we are on the tiled layer"
    );
    assert_eq!(
        state.agents_launches, 0,
        "the panel's missing half is not a half a layout pane owes anyone"
    );
    assert_eq!(
        state.last_placement, None,
        "and nothing was placed: `change_overlay_coordinates_batch` refuses a non-overlay id, and \
         refuses the whole batch with it"
    );
    assert!(
        state.visibility_calls.is_empty(),
        "nothing was shown or hidden either"
    );
    assert_eq!(
        state.own_tab_index,
        Some(0),
        "what it does still read off the manifest is where it is"
    );
}

/// Esc in a hocus the user put in their layout hides nothing. The pane is their arrangement, and
/// there would be no way back: the reveal bindings summon the overlay panel, not a tab pane.
#[test]
fn esc_in_a_layout_pane_hides_nothing() {
    let mut state = State {
        own_plugin_id: 7,
        kdl: state_config(),
        ..Default::default()
    };
    state.on_panes(manifest(0, vec![tiled_own(7)]));

    state.hide_panel();

    assert!(
        !state.hidden,
        "the pane the layout asked for stays where it is"
    );
    assert!(
        state.visibility_calls.is_empty(),
        "nothing was asked of the host"
    );
    assert_eq!(state.agents_launches, 0);
}

/// A panel that has been put away does not chase tab switches - either half of it. `agents_visible`
/// is set optimistically the moment a list announces itself, so reading it here as well made a
/// dismissed panel drag both suppressed panes to whatever tab the user switched to.
///
/// `break_panes_to_tab_with_index` panics off wasm, so "no move was attempted" is assertable.
#[test]
fn a_dismissed_panel_does_not_follow_a_tab_switch() {
    let mut state = State {
        own_is_floating: Some(true),
        own_plugin_id: 7,
        own_tab_index: Some(0),
        agents_plugin: Some(9),
        hidden: true,
        agents_visible: true,
        client: ClientId::new(1),
        client_tabs: BTreeMap::from([(ClientId::new(1), 0)]),
        ..Default::default()
    };

    state.on_tabs(vec![
        TabInfo {
            position: 0,
            tab_id: 0,
            active: false,
            display_area_rows: 50,
            display_area_columns: 200,
            ..TabInfo::default()
        },
        TabInfo {
            position: 1,
            tab_id: 1,
            active: true,
            display_area_rows: 50,
            display_area_columns: 200,
            ..TabInfo::default()
        },
    ]);

    assert_eq!(
        state.own_tab_index,
        Some(0),
        "the panel stays where it will be asked back from"
    );
}

/// The stale-hide suppression has an end, and this is it. A `c` pressed on the heels of the Esc
/// unsuppresses our pane from outside, which reaches hocus only as a manifest - and one of
/// exactly the shape a hide that has not landed yet produces. Nothing tells them apart, so the
/// budget does: a real hide lands within a frame or two, and the manifest after that is believed.
///
/// Held as a flag this never lapsed. The reveal was swallowed, so was every manifest after it,
/// and the panel sat on screen believing itself dismissed - `hidden` true, no list beside it, no
/// tab following - with only a second Esc to get out.
#[test]
fn a_hide_that_never_lands_stops_being_believed() {
    let mut state = agents_beside_state();
    state.hide_panel();

    // The frames a dismissal is entitled to: our pane still standing in every one of them.
    for _ in 0..HIDE_PENDING_FRAMES {
        state.on_panes(manifest(0, list_beside()));
        assert!(state.hidden, "the Esc is still being waited on");
    }

    state.on_panes(manifest(0, list_beside()));

    assert!(
        !state.hidden,
        "the budget is spent, so the manifest is read at face value and the user's `c` wins"
    );
    assert_eq!(state.hide_pending, 0);
}
