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
        waiting_rows(&rows),
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
    state.set_rows(strip(vec![agent(1, false)]));
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
        .contains("Enter read · Tab chat"));
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
