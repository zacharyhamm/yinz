use super::*;

fn agent(pane_id: u32, waiting: bool) -> StripRow {
    StripRow::Agent {
        pane_id,
        text: "a title containing waiting and !".to_owned(),
        kind: "codex".to_owned(),
        waiting,
    }
}

fn strip(rows: Vec<StripRow>) -> Strip {
    Strip {
        rows,
        ..Default::default()
    }
}

#[test]
fn attention_keeps_only_waiting_agents_and_their_headers() {
    let rows = vec![
        StripRow::Project("unused".into()),
        StripRow::Branch("idle".into()),
        agent(1, false),
        StripRow::Project("project".into()),
        StripRow::Branch("busy".into()),
        agent(2, false),
        StripRow::Branch("work".into()),
        agent(3, true),
        agent(4, true),
        StripRow::Branch("done".into()),
        agent(5, false),
        StripRow::Branch("review".into()),
        agent(6, true),
        StripRow::Project("archive".into()),
        StripRow::Run {
            run_id: "codex:archived".into(),
            kind: "codex".into(),
            title: "waiting in an archived title".into(),
            worktree: String::new(),
            started_at_ms: None,
            continuation: false,
        },
        StripRow::MoreRuns,
    ];
    assert_eq!(
        waiting_rows(&rows, None),
        [3, 6, 7, 8, 11, 12]
            .map(|index| rows[index].clone())
            .to_vec()
    );
}

#[test]
fn attention_tracks_live_updates_without_moving_a_surviving_selection_or_scroll() {
    let mut state = State::default();
    state.set_rows(strip(vec![agent(1, false), agent(2, true), agent(3, true)]));
    state.selected = 2;
    state.update(Event::Key(KeyWithModifier::new(BareKey::Char('w'))));
    assert_eq!(state.selection(), Some(3));
    assert_eq!(state.rows, vec![agent(2, true), agent(3, true)]);
    state.list_scroll = Some(1);
    state.history_scroll = 3;

    let refreshed = Strip {
        focused: Some(2),
        ..strip(vec![
            agent(1, false),
            agent(2, true),
            agent(3, true),
            agent(4, false),
        ])
    };
    assert!(!state.set_rows(refreshed.clone()));
    assert_eq!(
        state.selection(),
        Some(3),
        "background focus does not move the queue cursor"
    );
    assert_eq!(state.list_scroll, Some(1));
    assert_eq!(state.history_scroll, 3);

    state.update(Event::Key(KeyWithModifier::new(BareKey::Char('w'))));
    assert_eq!(
        state.rows, refreshed.rows,
        "leaving the filter restores the latest strip"
    );
    assert_eq!(state.selection(), Some(3));
    assert_eq!(state.list_scroll, None);
}

#[test]
fn attention_can_empty_and_repopulate_and_keeps_other_views_separate() {
    let mut state = State::default();
    state.set_rows(strip(vec![agent(1, true)]));
    state.toggle_waiting();
    // Answered, the agent under the cursor stays in the queue while the cursor is on it...
    state.set_rows(strip(vec![agent(1, false)]));
    assert_eq!(state.rows, vec![agent(1, false)]);
    // ...and it leaves with its pane.
    state.set_rows(strip(vec![agent(2, false)]));
    state.render(20, 70);
    assert!(state.rows.is_empty());
    assert_eq!(state.selection(), None);
    assert_eq!(state.last_history_rows, 0);
    assert!(state.footer_hint().unwrap().contains("w all · 0 waiting"));

    state.set_rows(strip(vec![agent(1, false), agent(2, true)]));
    assert_eq!(state.selection(), Some(2));
    assert!(state.footer_hint().unwrap().contains("1 waiting"));
    state.focus = Focus::History;
    assert!(state
        .footer_hint()
        .unwrap()
        .contains("1 waiting · ↑↓ event"));
    state.begin_predecessors();
    assert!(state.waiting_base.is_none());
    assert!(state.predecessors.is_some());
    state.toggle_waiting();
    assert!(state.predecessors.is_none());
    assert!(state.waiting_base.is_some());
    state.begin_search();
    assert!(state.waiting_base.is_none());
    assert!(state.search_editing);
    state.search_key(KeyWithModifier::new(BareKey::Esc));
    assert_eq!(state.rows, vec![agent(1, false), agent(2, true)]);
}

/// The waiting view names itself above the list, and keeps doing so across a hide - it is a
/// queue the user is working through, not a search to be thrown away. Empty, it says how to
/// get back to every agent and offers no key that would act on a row, and `c` leaves it the
/// way it leaves the other two views.
#[test]
fn the_waiting_view_says_what_it_is_and_the_way_back() {
    let mut state = State::default();
    state.set_rows(strip(vec![agent(1, false), agent(2, true)]));
    assert!(state.toggle_waiting());
    state.render(20, 70);
    assert_eq!(state.view_label().as_deref(), Some("Waiting for you"));
    assert_eq!(
        state.tab_cells[0].text, " Agents │",
        "the tab is still the Agents tab"
    );
    state.hide_the_panel();
    state.render(20, 70);
    assert_eq!(state.view_label().as_deref(), Some("Waiting for you"));

    // Its pane gone, the queue is empty.
    state.set_rows(strip(vec![agent(1, false)]));
    state.render(20, 70);
    assert!(state.rows.is_empty());
    assert_eq!(
        state.empty_list_message(),
        ("no agents waiting for you", Some("w shows every agent"))
    );
    let footer = state.footer_hint().unwrap();
    assert!(!footer.contains("Enter"), "{footer}");
    assert!(!footer.contains("Tab"), "{footer}");

    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('c'))));
    assert!(state.waiting_base.is_none());
    assert_eq!(state.rows, vec![agent(1, false)]);
}

/// The count in the footer is the session's, not the view's: a search shows no live agents to
/// count, and still says how many are waiting - straight after the way out of the search, and
/// ahead of the hit's own verb, at the widths a panel is drawn.
#[test]
fn the_waiting_count_survives_a_search_and_the_history_half() {
    let mut state = State::default();
    state.set_rows(strip(vec![agent(1, false), agent(2, true)]));
    state.render(20, 34);
    state.focus = Focus::History;
    state.render(20, 34);
    assert!(state.footer_hint().unwrap().contains("w waiting (1)"));

    state.focus = Focus::List;
    let search = |state: &mut State, hits: &[&str]| {
        state.begin_search();
        state.search_query = "anything".into();
        state.search_key(KeyWithModifier::new(BareKey::Enter));
        let result = wire::SearchResult {
            request_id: state.search_request.to_string(),
            runs: hits
                .iter()
                .map(|id| wire::SearchRun {
                    run_id: (*id).to_owned(),
                    kind: "codex".to_owned(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        assert!(state.apply_search_result(result));
    };
    search(&mut state, &[]);
    assert!(state.rows.is_empty());
    state.render(20, 36);
    assert_eq!(
        state.footer_hint().as_deref(),
        Some("Esc/c clear · w waiting (1) · / edit")
    );
    // The strip behind an empty search changes the count, so it asks for the redraw that shows
    // it; the plugin draws only what the pipe asks for. The same count asks for nothing.
    assert!(state.set_rows(strip(vec![agent(1, false), agent(2, false)])));
    assert!(!state.set_rows(strip(vec![agent(1, false), agent(2, false)])));
    state.render(20, 36);
    assert_eq!(state.footer_hint().as_deref(), Some("Esc/c clear · / edit"));
    assert!(state.set_rows(strip(vec![agent(1, false), agent(2, true)])));
    state.render(20, 36);
    assert_eq!(
        state.footer_hint().as_deref(),
        Some("Esc/c clear · w waiting (1) · / edit")
    );

    // A hit: the count, then the hit's verb, which is rendered for - it needs the history half
    // the first frame measures.
    assert!(state.clear_search());
    search(&mut state, &["codex:hit"]);
    let footer = |state: &mut State, cols: usize| {
        state.render(20, cols);
        state.render(20, cols);
        state.footer_hint().unwrap()
    };
    assert_eq!(footer(&mut state, 34), "Esc/c clear · w waiting (1)");
    // Nobody waiting any more, said by a strip that arrives during the search - which asks for
    // the redraw, or the footer would keep the old count until the next key.
    assert!(state.set_rows(strip(vec![agent(1, false), agent(2, false)])));
    assert!(!state.set_rows(strip(vec![agent(1, false), agent(2, false)])));
    assert_eq!(footer(&mut state, 34), "Esc/c clear · Enter history");
    let wide = footer(&mut state, 200);
    assert!(
        wide.starts_with("Esc/c clear · Enter history · r resume"),
        "{wide}"
    );
    assert!(wide.contains("/ edit"), "{wide}");
}

/// The agent under the cursor stays in the queue after it stops waiting, until the cursor
/// leaves it: dropping it at once moved the cursor to the top of the queue and closed the
/// reader and history the user had open on it.
#[test]
fn attention_keeps_the_selected_agent_after_it_stops_waiting_until_the_cursor_leaves() {
    let mut state = State::default();
    let asking = |waiting: bool| Strip {
        summaries: Summaries::from([(
            3,
            agents::Summary {
                text: "Shall I push?".to_owned(),
                title: "pushing".to_owned(),
                awaiting: waiting,
                hash: 1,
                created_at_ms: None,
            },
        )]),
        ..strip(vec![agent(3, waiting), agent(7, true), agent(9, true)])
    };
    state.set_rows(asking(true));
    assert!(state.toggle_waiting());
    assert_eq!(state.selection(), Some(3));
    state.render(21, 50);
    state.focus = Focus::History;
    assert!(state.on_key(KeyWithModifier::new(BareKey::Enter)));
    assert!(state.reading.is_some());

    // Answered from somewhere else: the row stays, and so does what is open on it.
    let was = state.selection_key();
    state.set_rows(asking(false));
    assert!(!state.follow_selection(was));
    assert_eq!(state.selection(), Some(3));
    assert!(state.reading.is_some());
    state.close_reader();
    state.fill_history_cache(50);
    assert!(state.history_label().ends_with("no longer waiting"));

    // The cursor moves on, and the next strip lets the row go.
    state.focus = Focus::List;
    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('j'))));
    assert_eq!(state.selection(), Some(7));
    state.set_rows(asking(false));
    assert!(position_of(&state.rows, 3).is_none());
    assert_eq!(state.selection(), Some(7));
}

/// A row that has gone altogether - its pane closed - hands the cursor to its neighbour in the
/// queue, not back to the top of it.
#[test]
fn attention_falls_back_to_the_neighbour_when_the_selected_pane_exits() {
    let mut state = State::default();
    state.set_rows(strip(vec![agent(3, true), agent(7, true), agent(9, true)]));
    assert!(state.toggle_waiting());
    state.selected = position_of(&state.rows, 7).unwrap();
    state.set_rows(strip(vec![agent(3, true), agent(9, true)]));
    assert_eq!(state.selection(), Some(9));
}

/// Opening the view from a row that is not waiting starts at the top of the queue. The cursor
/// used to be placed by its index in the whole list, applied to the queue, and landed on an
/// arbitrary waiting agent - from the foot of the list, always the last.
#[test]
fn opening_the_view_off_the_queue_starts_at_its_top() {
    for from in [3, 4] {
        let mut state = State::default();
        state.set_rows(strip(vec![
            StripRow::Project("p".into()),
            agent(1, true),
            agent(2, true),
            agent(3, false),
            agent(4, false),
        ]));
        state.selected = from;
        state.update(Event::Key(KeyWithModifier::new(BareKey::Char('w'))));
        assert_eq!(state.selection(), Some(1), "from row {from}");
    }
}
