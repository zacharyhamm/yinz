use super::*;

#[test]
fn companion_visibility_tracks_the_panel_without_giving_agents_the_keyboard() {
    let mut state = State::default();
    let companion = PaneId::Plugin(PluginId::new(1));
    let tabs = PaneTabsInfo {
        id: PaneTabsId(7),
        pane_id: companion,
        tabs: state
            .composition_tabs()
            .into_iter()
            .enumerate()
            .map(|(index, descriptor)| PaneTabInfo {
                terminal_id: None,
                descriptor,
                plugin_id: Some(PluginId::new(index as u32)),
                state: PaneTabLoadState::Ready,
            })
            .collect(),
        active: 1,
        bar_focused: vec![],
    };
    let mut manifest = PaneManifest {
        panes: std::collections::HashMap::from([(
            TabPosition::new(0),
            vec![
                PaneInfo {
                    id: PaneId::Plugin(state.own_plugin_id),
                    is_suppressed: true,
                    pane_tabs: Some(tabs.clone()),
                    ..Default::default()
                },
                PaneInfo {
                    id: companion,
                    is_floating: true,
                    is_focused: true,
                    pane_tabs: Some(tabs),
                    ..Default::default()
                },
            ],
        )]),
    };
    assert_eq!(
        panel_pane(&manifest, state.own_plugin_id).unwrap().id,
        companion
    );
    state.update(Event::PaneUpdate(manifest.clone()));
    assert!(state.pane_shown);
    assert!(!state.keys_here());
    assert_eq!(state.panel_hides, 0);
    let mut departed = manifest.clone();
    let panes = departed.panes.get_mut(&TabPosition::new(0)).unwrap();
    panes[1].is_focused = false;
    panes.push(PaneInfo {
        id: PaneId::Terminal(TerminalId::new(2)),
        is_focused: true,
        ..Default::default()
    });
    state.update(Event::PaneUpdate(departed));
    assert_eq!(
        state.panel_hides, 0,
        "the host dismisses on focus departure; an older update must not hide again"
    );
    manifest.panes.get_mut(&TabPosition::new(0)).unwrap()[1].is_suppressed = true;
    state.update(Event::PaneUpdate(manifest));
    assert!(!state.pane_shown);
}

#[test]
fn enter_on_more_runs_redraws_synchronously_refreshed_rows() {
    let mut state = State {
        snapshot: Some(wire::PollReply {
            panes: None,
            suppressed_panes: vec![],
            activity: Default::default(),
            manual_kinds: Default::default(),
            yinz_session_name: Default::default(),
            repositories: Default::default(),
            tab_projects: Default::default(),
            focused: None,
            summaries: Default::default(),
            hooks: Default::default(),
            runs: vec![],
        }),
        archived_runs: vec![a_run().header()],
        rows: vec![StripRow::MoreRuns],
        ..Default::default()
    };
    assert!(state.on_key(KeyWithModifier::new(BareKey::Enter)));
    assert!(state
        .rows
        .iter()
        .any(|row| matches!(row, StripRow::Run { .. })));
    assert_eq!(state.archive_requests, 1);
    assert_eq!(state.panel_hides, 0);
}

#[test]
fn pane_tabs_descriptors_keep_originating_cwd_on_retries() {
    let origin = std::path::PathBuf::from("/tmp/agent-origin");
    assert_ne!(origin, std::env::current_dir().unwrap());
    let state = State {
        initial_cwd: origin.clone(),
        ..Default::default()
    };
    for _ in 0..2 {
        let tabs = state.composition_tabs();
        assert_eq!(
            tabs.iter()
                .map(|tab| tab.location.as_str())
                .collect::<Vec<_>>(),
            ["yinz:agents", "yinz:todos"]
        );
        assert_eq!(
            tabs[0].configuration.get("view").map(String::as_str),
            Some("agents")
        );
        assert!(tabs.iter().skip(1).all(|tab| tab.configuration.is_empty()));
        assert!(tabs
            .iter()
            .all(|tab| tab.initial_cwd.as_ref() == Some(&origin)));
    }
}

/// The row an `s` is waiting on, if any.
fn asked(state: &State) -> Option<&str> {
    state.summarizing.as_ref().map(|ask| ask.row.as_str())
}

fn search_reply(state: &mut State, ids: &[&str]) -> wire::SearchResult {
    state.begin_search();
    state.search_query = "history".to_owned();
    state.search_key(KeyWithModifier::new(BareKey::Enter));
    assert!(state.search_pending);
    wire::SearchResult {
        request_id: state.search_request.to_string(),
        runs: ids
            .iter()
            .map(|id| wire::SearchRun {
                run_id: (*id).to_owned(),
                kind: "claude".to_owned(),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
}

fn predecessor_reply(state: &State, ids: &[&str]) -> wire::PredecessorsResult {
    let mut anchor = a_run().header();
    anchor.id = "anchor".into();
    wire::PredecessorsResult {
        request_id: state.predecessor_request.to_string(),
        pane_id: state.predecessors.as_ref().unwrap().pane_id,
        anchor: Some(anchor),
        runs: ids
            .iter()
            .map(|id| {
                let mut header = a_run().header();
                header.id = (*id).into();
                header
            })
            .collect(),
        diagnostic: String::new(),
    }
}

#[test]
fn predecessors_select_history_keep_live_updates_and_restore_latest_archive() {
    let mut state = State::default();
    let mut rows = strip();
    rows.extend(archive());
    state.set_rows(unfocused(rows));
    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('p'))));
    let reply = predecessor_reply(&state, &["nearest", "oldest"]);
    assert!(state.apply_predecessors(reply));
    assert_eq!(state.selected_run_id().as_deref(), Some("nearest"));
    assert_eq!(state.selected_revision(), None);
    assert!(!state
        .rows
        .iter()
        .any(|row| matches!(row, StripRow::MoreRuns)));
    let mut run = a_run();
    run.id = "nearest".into();
    assert!(state.fixture_pipe(PipeMessage::new(
        PipeSource::Plugin(yinz_tile::prelude::PluginId::new(0)),
        "agent_run_detail",
        &Some(serde_json::to_string(&run).unwrap()),
        &None,
        false
    )));
    assert_eq!(state.run.as_ref().unwrap().id, "nearest");
    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('r'))));
    assert_eq!(state.confirming_resume.as_deref(), Some("run:nearest"));
    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('n'))));
    assert!(state.step_selection(Motion::Down));
    assert_eq!(state.selected_run_id().as_deref(), Some("oldest"));
    let mut incoming = with_summary(strip(), 7, "new summary", false);
    incoming.rows[2] = agent(3, "new run after /clear");
    incoming.rows.push(agent(99, "new live agent"));
    incoming.rows.extend(archive());
    incoming.focused = Some(99);
    state.set_rows(incoming.clone());
    assert_eq!(state.selected_run_id().as_deref(), Some("oldest"));
    assert!(position_of(&state.rows, 99).is_some());
    assert_eq!(state.summaries, incoming.summaries);
    assert_eq!(
        state
            .predecessors
            .as_ref()
            .unwrap()
            .result
            .as_ref()
            .unwrap()
            .anchor
            .as_ref()
            .unwrap()
            .id,
        "anchor"
    );
    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('c'))));
    assert_eq!(state.rows, incoming.rows);
    assert_eq!(state.summaries, incoming.summaries);
}

#[test]
fn predecessors_replacement_clear_search_and_stale_replies() {
    for action in ["replace", "clear", "search"] {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        state.begin_predecessors();
        let stale = predecessor_reply(&state, &["stale"]);
        match action {
            "replace" => {
                state.selected = position_of(&state.rows, 7).unwrap();
                assert!(state.begin_predecessors());
            },
            "clear" => {
                state.clear_predecessors();
            },
            _ => {
                state.begin_search();
            },
        }
        assert!(!state.apply_predecessors(stale));
        if action == "replace" {
            let reply = predecessor_reply(&state, &["replacement"]);
            assert!(state.apply_predecessors(reply));
            assert_eq!(state.selected_run_id().as_deref(), Some("replacement"));
        }
    }
    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    let stale = search_reply(&mut state, &["stale"]);
    assert!(state.begin_predecessors());
    assert!(state.search_base.is_none());
    assert!(!state.apply_search_result(stale));
    let empty = predecessor_reply(&state, &[]);
    assert!(state.apply_predecessors(empty.clone()));
    assert!(!state.apply_predecessors(empty));
    assert!(state
        .rows
        .iter()
        .any(|row| matches!(row, StripRow::Project(label) if label == "No predecessors recorded")));
    // Esc steps back out of the view first, and only then is the dismissal it always was.
    assert!(state.on_key(KeyWithModifier::new(BareKey::Esc)));
    assert!(
        state.predecessors.is_none(),
        "the first Escape leaves the view"
    );
    assert_eq!(state.panel_hides, 0);
    assert!(!state.on_key(KeyWithModifier::new(BareKey::Esc)));
    assert_eq!(state.panel_hides, 1, "and the second puts the panel away");
    state.begin_predecessors();
    let mut partial = predecessor_reply(&state, &["reachable"]);
    partial.diagnostic = "Incomplete chain: missing run lost".into();
    assert!(state.apply_predecessors(partial));
    assert_eq!(state.selected_run_id().as_deref(), Some("reachable"));
    assert!(state
        .rows
        .iter()
        .any(|row| matches!(row, StripRow::Project(label) if label.contains("Incomplete chain"))));
}

#[test]
fn search_revisions_reposition_with_list_focus_and_refresh_preserves_scroll() {
    let mut state = State::default();
    state.set_rows(one_run_selected());
    assert_eq!(state.selected_revision(), None);
    let mut run = a_run();
    run.entries = (1..=600)
        .map(|revision| agents::RunEntry {
            tool: None,
            revision,
            text: format!("message {revision}"),
            kind: agents::RunEntryKind::Message,
            created_at_ms: Some(revision as i64),
            event_id: None,
        })
        .collect();
    state.focus = Focus::List;
    for revision in [10, 20] {
        let mut reply = search_reply(&mut state, &[&run.id]);
        reply.runs[0].revision = revision;
        assert!(state.apply_search_result(reply));
        assert_eq!(state.selected_revision(), Some(revision));
        // A detail already in flight for the preceding hit must not settle this anchor.
        assert!(!state.fixture_pipe(PipeMessage::new(
            PipeSource::Plugin(yinz_tile::prelude::PluginId::new(0)),
            "agent_run_detail",
            &Some(serde_json::to_string(&run).unwrap()),
            &Some(BTreeMap::from([(
                "revision".to_owned(),
                (revision - 1).to_string()
            )])),
            false,
        )));
        state.render(39, 80);
        assert_eq!(state.pending_history_revision, Some(revision));
        state.fixture_pipe(PipeMessage::new(
            PipeSource::Plugin(yinz_tile::prelude::PluginId::new(0)),
            "agent_run_detail",
            &Some(serde_json::to_string(&run).unwrap()),
            &Some(BTreeMap::from([(
                "revision".to_owned(),
                revision.to_string(),
            )])),
            false,
        ));
        state.render_history(0, 0, 80);
        assert_eq!(state.pending_history_revision, Some(revision));
        state.render(39, 80);
        assert_eq!(state.cursor_event().unwrap().revision, Some(revision));
        let event = state.cursor_event().unwrap();
        let end = state.history().entries.len() - state.history_scroll;
        assert!(
            event.start < end && event.end() > end.saturating_sub(state.last_history_body_rows)
        );
        state.scroll_history(-3);
        let scroll = state.history_scroll;
        run.entries.last_mut().unwrap().text.push('!');
        state.fixture_pipe(PipeMessage::new(
            PipeSource::Plugin(yinz_tile::prelude::PluginId::new(0)),
            "agent_run_detail",
            &Some(serde_json::to_string(&run).unwrap()),
            &Some(BTreeMap::from([(
                "revision".to_owned(),
                revision.to_string(),
            )])),
            false,
        ));
        state.render(39, 80);
        assert_eq!(state.history_scroll, scroll);
    }
    state.clear_search();
    assert_eq!(state.selected_revision(), None);
}

#[test]
fn cancelling_an_initial_search_restores_live_rows_and_summaries() {
    for pending in [false, true] {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('/'))));
        let reply = pending.then(|| search_reply(&mut state, &["stale"]));
        let mut incoming = with_summary(strip(), 7, "While editing", false);
        incoming.rows.push(agent(10, "new agent"));
        state.set_rows(incoming.clone());
        assert!(state.on_key(KeyWithModifier::new(BareKey::Esc)));
        assert!(state.search_base.is_none());
        assert!(!state.search_editing);
        assert!(!state.search_pending);
        assert!(state.search_query.is_empty());
        assert_eq!(state.rows, incoming.rows);
        assert_eq!(state.summaries, incoming.summaries);
        if let Some(reply) = reply {
            assert!(!state.apply_search_result(reply));
        }
        incoming.rows.push(agent(11, "another agent"));
        incoming.summaries.get_mut(&7).unwrap().text = "After Escape".to_owned();
        assert!(state.set_rows(incoming.clone()));
        assert_eq!(state.rows, incoming.rows);
        assert_eq!(state.summaries, incoming.summaries);
    }
}

#[test]
fn cancelling_edits_restores_the_applied_query_and_matches() {
    for pending in [false, true] {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        let reply = search_reply(&mut state, &["match-1", "match-2"]);
        assert!(state.apply_search_result(reply));
        state.selected = 1;
        let matches = state.rows.clone();
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('/'))));
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('x'))));
        if pending {
            assert!(state.on_key(KeyWithModifier::new(BareKey::Enter)));
        }
        let stale = wire::SearchResult {
            request_id: state.search_request.to_string(),
            ..Default::default()
        };
        assert!(state.on_key(KeyWithModifier::new(BareKey::Esc)));
        assert_eq!(state.search_query, "history");
        assert_eq!(state.applied_search_query.as_deref(), Some("history"));
        assert_eq!(state.rows, matches);
        assert_eq!(state.selected, 1);
        assert!(!state.search_editing);
        assert!(!state.search_pending);
        assert!(!state.apply_search_result(stale));
        state.set_rows(unfocused(vec![agent(10, "new agent")]));
        assert_eq!(state.rows, matches);
        assert!(state.clear_search());
        assert!(state.applied_search_query.is_none());
    }
}

#[test]
fn editing_a_pending_query_ignores_stale_replies_and_accepts_the_revised_query() {
    for key in [BareKey::Char('x'), BareKey::Backspace] {
        for error in ["", "stale error"] {
            let mut state = State::default();
            state.set_rows(unfocused(strip()));
            let original = state.rows.clone();
            let mut stale = search_reply(&mut state, &["match"]);
            stale.error = error.to_owned();
            assert!(state.on_key(KeyWithModifier::new(key)));
            let revised = state.search_query.clone();
            assert_eq!(
                revised,
                if key == BareKey::Backspace {
                    "histor"
                } else {
                    "historyx"
                }
            );
            assert!(!state.search_pending);
            assert!(!state.apply_search_result(stale.clone()));
            assert!(state.search_editing);
            assert_eq!(state.rows, original);
            assert!(state.search_error.is_none());
            assert!(state.on_key(KeyWithModifier::new(BareKey::Enter)));
            assert!(!state.apply_search_result(stale.clone()));
            assert!(state.search_pending);
            stale.request_id = state.search_request.to_string();
            stale.error.clear();
            assert!(state.apply_search_result(stale));
            assert_eq!(state.search_query, revised);
            assert_eq!(
                state.applied_search_query.as_deref(),
                Some(revised.as_str())
            );
            assert_eq!(state.selection_key().as_deref(), Some("run:match"));
            assert!(!state.search_editing);
            assert!(state.search_error.is_none());
        }
    }
}

#[test]
fn strips_during_search_save_the_latest_rows_without_replacing_matches() {
    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    let reply = search_reply(&mut state, &["match-1", "match-2"]);
    assert!(state.apply_search_result(reply));
    state.selected = 1;
    let matches = state.rows.clone();
    let mut incoming = with_summary(strip(), 7, "Updated summary", false);
    incoming.rows.push(agent(10, "new agent"));
    incoming.focused = Some(7);
    assert!(!state.set_rows(incoming.clone()));
    assert_eq!(state.rows, matches);
    assert_eq!(state.selected, 1);
    assert_eq!(state.focused, Some(7));
    state.set_rows(Strip {
        focused: None,
        ..incoming.clone()
    });
    assert_eq!(state.focused, Some(7));
    assert!(state.clear_search());
    assert_eq!(state.rows, incoming.rows);
    assert_eq!(state.summaries, incoming.summaries);
    assert!(state.search_base.is_none());
}

#[test]
fn cancelled_and_replaced_search_replies_are_ignored() {
    for cancellation in ["escape", "clear", "replace", "submit"] {
        for error in ["", "stale error"] {
            let mut state = State::default();
            state.set_rows(one_run_selected());
            let mut reply = search_reply(&mut state, &["stale"]);
            reply.error = error.to_owned();
            match cancellation {
                "escape" => {
                    state.search_key(KeyWithModifier::new(BareKey::Esc));
                },
                "clear" => {
                    state.clear_search();
                },
                "replace" => {
                    state.begin_search();
                },
                _ => {
                    search_reply(&mut state, &["new"]);
                },
            }
            let rows = state.rows.clone();
            let editing = state.search_editing;
            let pending = state.search_pending;
            assert!(!state.apply_search_result(reply));
            assert_eq!(state.rows, rows);
            assert_eq!(state.search_editing, editing);
            assert_eq!(state.search_pending, pending);
            assert!(state.search_error.is_none());
        }
    }
}

#[test]
fn search_results_follow_selection_and_preserve_state_for_the_same_run() {
    for id in ["claude:session-1", "another-run"] {
        let mut state = reading("An open event");
        state.history_scroll = 3;
        state.history_cursor = 2;
        state.summarizing = state.selection_key().map(|row| SummaryAsk {
            row,
            asked_at_ms: 0,
            stale: false,
        });
        state.confirming_resume = state.selection_key();
        state.resume_note = Some((state.selection_key().unwrap(), "refused".to_owned()));
        state
            .search_revisions
            .insert("claude:session-1".to_owned(), 0);
        let reply = search_reply(&mut state, &[id]);
        assert!(state.apply_search_result(reply));
        let same = id == "claude:session-1";
        assert_eq!(state.history_scroll, if same { 3 } else { 0 });
        assert_eq!(state.history_cursor, if same { 2 } else { 0 });
        assert_eq!(state.reading.is_some(), same);
        assert_eq!(state.reader_cache.is_some(), same);
        assert_eq!(state.summarizing.is_some(), same);
        assert_eq!(state.confirming_resume.is_some(), same);
        assert_eq!(state.resume_note.is_some(), same);
    }
}

#[test]
fn empty_results_and_errors_finish_search_and_ignore_duplicate_replies() {
    for error in ["", "search failed"] {
        let mut state = reading("An open event");
        let original = state.rows.clone();
        let mut reply = search_reply(&mut state, &[]);
        reply.error = error.to_owned();
        reply.truncated = true;
        assert!(state.apply_search_result(reply.clone()));
        // A failure keeps the box open on the query, for Enter to retry and Esc to leave.
        assert_eq!(state.search_editing, !error.is_empty());
        assert!(!state.search_pending);
        assert!(state.search_base.is_some());
        // A failure says nothing about how many matches there were: the rows below are the
        // last search's, and so is whether it left some out.
        assert_eq!(state.search_truncated, error.is_empty());
        assert_eq!(
            state.search_error.as_deref(),
            (!error.is_empty()).then_some(error)
        );
        if error.is_empty() {
            assert!(state.rows.is_empty());
            assert_eq!(state.selected, 0);
            assert!(state.reading.is_none());
        } else {
            assert_eq!(state.rows, original);
            assert!(state.reading.is_some());
        }
        assert!(!state.apply_search_result(reply));
        assert!(state.clear_search());
        assert_eq!(state.rows, original);
        assert!(!state.search_truncated);
        assert!(state.search_error.is_none());
    }
}

/// A refinement that fails leaves the search it was refining exactly as it was, including
/// the notice that it had more matches than it showed: `Esc` goes back to that search, and
/// the rows under it are still its rows.
#[test]
fn a_failed_refinement_keeps_the_applied_searchs_omission_notice() {
    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    let mut reply = search_reply(&mut state, &["match-1", "match-2"]);
    reply.truncated = true;
    assert!(state.apply_search_result(reply));
    let label = state.view_label().expect("a search view");
    assert!(label.contains("more matches omitted"), "{label}");

    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('/'))));
    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('x'))));
    assert!(state.on_key(KeyWithModifier::new(BareKey::Enter)));
    let failed = wire::SearchResult {
        request_id: state.search_request.to_string(),
        error: "store busy".into(),
        ..Default::default()
    };
    assert!(state.apply_search_result(failed));
    assert!(state.on_key(KeyWithModifier::new(BareKey::Esc)));
    assert_eq!(state.view_label(), Some(label));
}

fn agent(pane_id: u32, text: &str) -> StripRow {
    StripRow::Agent {
        pane_id,
        text: text.to_owned(),
        waiting: false,
        // the second field of the line, the way agent spells it
        kind: text
            .split_whitespace()
            .nth(1)
            .unwrap_or("claude")
            .to_owned(),
    }
}

/// An `AGENT_STRIP_TAKE` naming `stop`, as agent sends it when the ring steps in.
fn take(stop: &str) -> PipeMessage {
    PipeMessage::new(
        PipeSource::Plugin(yinz_tile::prelude::PluginId::new(0)),
        wire::AGENT_STRIP_TAKE,
        &None,
        &Some(BTreeMap::from([(
            wire::PANEL_STOP.to_owned(),
            stop.to_owned(),
        )])),
        false,
    )
}

fn strip() -> Vec<StripRow> {
    vec![
        StripRow::Project("agent".to_owned()),
        StripRow::Branch("  main".to_owned()),
        agent(3, "  3  claude  busy"),
        agent(7, "  7  codex  idle"),
        StripRow::Project("no project".to_owned()),
        agent(9, "  9  claude  held"),
    ]
}

/// A push from agent with nobody in a terminal pane - what one of the agent
/// panes having the focus looks like on the wire.
fn unfocused(rows: Vec<StripRow>) -> Strip {
    Strip {
        rows,
        focused: None,
        summaries: Summaries::new(),
    }
}

/// A push from agent with `pane_id` focused.
fn focused_on(rows: Vec<StripRow>, pane_id: u32) -> Strip {
    Strip {
        rows,
        focused: Some(pane_id),
        summaries: Summaries::new(),
    }
}

/// A push carrying one summary, for the box under the list.
fn with_summary(rows: Vec<StripRow>, pane_id: u32, text: &str, awaiting: bool) -> Strip {
    Strip {
        rows,
        focused: None,
        summaries: Summaries::from([(
            pane_id,
            agents::Summary {
                text: text.to_owned(),
                title: "running the tests".to_owned(),
                awaiting,
                hash: 1,
                created_at_ms: None,
            },
        )]),
    }
}

/// The ages are drawn from the clock, so the minute tick is the only thing that can
/// refresh them - and it redraws only where there is an age on screen, so a pane showing
/// nothing time-dependent still costs nothing to leave open.
#[test]
fn the_minute_tick_redraws_only_where_an_age_is_shown() {
    let mut state = State::default();
    state.set_rows(with_summary(
        strip(),
        3,
        "Waiting on a permission prompt.",
        true,
    ));
    assert!(
        !state.shows_an_age(),
        "a summary made before there were stamps has no age to move"
    );

    let mut stamped = with_summary(strip(), 3, "Waiting on a permission prompt.", true);
    stamped
        .summaries
        .get_mut(&3)
        .expect("the summary just pushed")
        .created_at_ms = Some(1);
    state.set_rows(stamped);
    assert!(state.shows_an_age());

    // And a run, whose entries wear their ages in a column of their own.
    state.set_run(a_run());
    state.set_rows(unfocused(vec![
        StripRow::Project("agent".to_owned()),
        run_row("claude:session-1".to_owned(), "hook history"),
    ]));
    assert!(!state.shows_an_age(), "nothing in `a_run` is stamped");
    let mut stamped = a_run();
    stamped
        .entries
        .first_mut()
        .expect("a timeline")
        .created_at_ms = Some(1);
    state.set_run(stamped);
    assert!(state.shows_an_age());

    // And the archive's own age column, which is drawn in the *list* half: it has to tick
    // with nothing stamped in the history half beside it, which is the usual case for an
    // archived run that was never summarized.
    let mut state = State::default();
    state.set_rows(unfocused(vec![
        StripRow::Project("archive".to_owned()),
        run_row("claude:session-1".to_owned(), "hook history"),
    ]));
    assert!(
        !state.shows_an_age(),
        "a row that cannot say when it started draws a blank gutter, not an age"
    );
    let StripRow::Run { started_at_ms, .. } = &mut state.rows[1] else {
        panic!("not a run row");
    };
    *started_at_ms = Some(1);
    assert!(state.shows_an_age());
}

/// The once-a-second push that moves nothing must cost nothing: `generation` is what
/// throws the history cache away, and the cache exists to keep a settled pane from
/// re-parsing a summary's markdown every second.
///
/// A summary carries the time it was made now, and `Summary` derives `PartialEq`, so a
/// daemon that re-stamped an unchanged summary would move `generation` on every poll.
/// `agents::stamp_summary` is what stops that; this is the pane's half of the same claim.
#[test]
fn a_summary_that_says_the_same_thing_does_not_bounce_the_history_cache() {
    let mut state = State::default();
    let push = || with_summary(strip(), 3, "Waiting on a permission prompt.", true);
    assert!(state.set_rows(push()));
    state.fill_history_cache(48);
    let generation = state.generation;
    let cached = state.history().clone();

    assert!(
        !state.set_rows(push()),
        "the same rows and the same summary said nothing"
    );
    assert_eq!(state.generation, generation);
    state.fill_history_cache(48);
    assert_eq!(state.history(), &cached, "and the cache survived it");
}

#[test]
fn a_fresh_strip_lands_on_the_first_agent_not_the_header() {
    let mut state = State::default();
    assert!(state.set_rows(unfocused(strip())));
    assert_eq!(state.selected, 2);
}

#[test]
fn stepping_skips_headers_in_both_directions() {
    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    state.selected = step(&state.rows, state.selected, Motion::Down);
    assert_eq!(state.selected, 3);
    state.selected = step(&state.rows, state.selected, Motion::Down);
    assert_eq!(state.selected, 5, "down from 7 steps over the header to 9");
    state.selected = step(&state.rows, state.selected, Motion::Up);
    assert_eq!(state.selected, 3);
    state.selected = step(&state.rows, state.selected, Motion::Up);
    state.selected = step(&state.rows, state.selected, Motion::Up);
    assert_eq!(state.selected, 2, "up from the first agent stays put");
}

/// What agent is told, and the reason the keys above are worth a message:
/// the selection names an agent, and agent draws that agent's summary.
#[test]
fn the_selection_is_the_agent_under_the_cursor() {
    let mut state = State::default();
    // Nothing on the list at all, and nothing to name.
    assert_eq!(state.selection(), None);

    state.set_rows(unfocused(strip()));
    assert_eq!(state.selection(), Some(3));
    state.selected = step(&state.rows, state.selected, Motion::Down);
    assert_eq!(state.selection(), Some(7));

    // A header cannot be walked onto, but a strip of nothing else can arrive.
    state.set_rows(unfocused(vec![StripRow::Project("agent".to_owned())]));
    assert_eq!(state.selection(), None);
}

#[test]
fn the_arrows_are_a_motion_whatever_is_held_with_them() {
    assert_eq!(
        arrow(&KeyWithModifier::new(BareKey::Down)),
        Some(Motion::Down)
    );
    assert_eq!(arrow(&KeyWithModifier::new(BareKey::Up)), Some(Motion::Up));
    assert_eq!(
        arrow(&KeyWithModifier::new(BareKey::Up).with_shift_modifier()),
        Some(Motion::Up)
    );
    // `j`/`k` are answered in `agents_key`, not here; see `arrow`.
    assert_eq!(arrow(&KeyWithModifier::new(BareKey::Char('j'))), None);
}

#[test]
fn a_click_resolves_to_the_agent_row_under_it() {
    let rows = strip();
    // line 3 is "  7  codex  idle", offset by a window that starts at 1
    assert_eq!(click_target(&rows, 1, rows.len(), 2), Some(3));
}

#[test]
fn a_click_on_a_header_row_hits_nothing() {
    let rows = strip();
    assert_eq!(
        click_target(&rows, 0, rows.len(), 0),
        None,
        "the Project header"
    );
    assert_eq!(
        click_target(&rows, 0, rows.len(), 1),
        None,
        "the Branch header"
    );
}

#[test]
fn a_click_past_the_last_row_hits_nothing() {
    let rows = strip();
    assert_eq!(click_target(&rows, 0, rows.len(), rows.len()), None);
}

/// `j`/`k` want the letter on its own, leaving `Ctrl j` and friends to whatever binds
/// them.
#[test]
fn a_modified_or_unrelated_letter_moves_nothing() {
    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    for key in [
        KeyWithModifier::new(BareKey::Char('j')).with_ctrl_modifier(),
        KeyWithModifier::new(BareKey::Char('k')).with_alt_modifier(),
        KeyWithModifier::new(BareKey::Char('x')),
    ] {
        assert!(!state.on_key(key));
        assert_eq!(state.selected, 2);
    }
}

/// Ctrl-o is agent's to answer - the list has no geometry of its own - so all this pane does
/// is ask, and take the key off whatever would have typed it.
#[test]
fn ctrl_o_is_taken_and_moves_the_selection_nowhere() {
    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    state.focus = Focus::History;

    assert!(
        !state.on_key(KeyWithModifier::new(BareKey::Char('o')).with_ctrl_modifier()),
        "nothing to redraw here - the move is the server's to paint"
    );

    assert_eq!(state.selected, 2, "the cursor stays where it was");
    assert_eq!(state.focus, Focus::History, "and so does the focus");
}

#[test]
fn jk_walk_the_selection_the_way_the_arrows_do() {
    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('j'))));
    assert_eq!(state.selected, 3);
    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('k'))));
    assert_eq!(state.selected, 2);
    assert!(
        !state.on_key(KeyWithModifier::new(BareKey::Char('k'))),
        "no move means nothing to redraw"
    );
    assert_eq!(state.selected, 2);
}

#[test]
fn a_refresh_follows_the_selected_pane_not_the_row_number() {
    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    state.selected = 3; // pane 7
    let mut rows = strip();
    // a new project appears above, shifting every row down
    rows.insert(0, StripRow::Project("aardvark".to_owned()));
    state.set_rows(unfocused(rows));
    assert!(matches!(
        state.rows.get(state.selected),
        Some(StripRow::Agent { pane_id: 7, .. })
    ));
}

#[test]
fn a_refresh_that_drops_the_selected_pane_falls_back_to_the_first_agent() {
    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    state.selected = 5; // pane 9
    let rows = vec![
        StripRow::Project("agent".to_owned()),
        agent(3, "  3  claude"),
    ];
    state.set_rows(unfocused(rows));
    assert_eq!(state.selected, 1);
}

#[test]
fn walking_into_an_agent_pane_moves_the_selection_onto_it() {
    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    assert_eq!(state.selected, 2, "pane 3, for want of anything better");

    assert!(state.set_rows(focused_on(strip(), 9)));
    assert_eq!(state.selected, 5);
    assert!(state.live(), "the selected agent is the one we are in");
}

#[test]
fn focus_on_something_that_is_not_an_agent_leaves_the_selection_standing() {
    let mut state = State::default();
    state.set_rows(focused_on(strip(), 7));
    assert_eq!(state.selected, 3);

    // pane 4 is a shell, an editor, anything at all - it is in no row
    assert!(state.set_rows(focused_on(strip(), 4)));
    assert_eq!(state.selected, 3, "still the agent we were last in");
    assert!(!state.live(), "but we are no longer in it");
}

#[test]
fn focus_on_a_plugin_pane_changes_nothing_at_all() {
    let mut state = State::default();
    state.set_rows(focused_on(strip(), 7));

    // agent sends `None` when a plugin pane has the focus, which is what
    // coming to this very list to work it looks like
    assert!(!state.set_rows(unfocused(strip())));
    assert_eq!(state.selected, 3);
    assert!(state.live(), "walking into the list is not leaving pane 7");
}

#[test]
fn a_push_that_does_not_move_the_focus_leaves_a_jk_selection_alone() {
    let mut state = State::default();
    state.set_rows(focused_on(strip(), 3));
    state.on_key(KeyWithModifier::new(BareKey::Char('j')));
    assert_eq!(state.selected, 3, "pane 7");

    // the snapshot pushes that keep arriving must not drag the cursor back
    state.set_rows(focused_on(strip(), 3));
    assert_eq!(state.selected, 3);
    assert!(!state.live(), "the cursor left the pane we are in");
}

#[test]
fn stepping_off_the_focused_agent_and_back_takes_the_liveness_with_it() {
    let mut state = State::default();
    state.set_rows(focused_on(strip(), 7));
    assert!(state.live());

    state.on_key(KeyWithModifier::new(BareKey::Char('j')));
    assert!(!state.live(), "selected pane 9, sitting in pane 7");
    state.on_key(KeyWithModifier::new(BareKey::Char('k')));
    assert!(state.live());
}

/// A redraw is owed for liveness alone: the rows and the selection can both
/// be identical while the band goes from coloured to dim.
#[test]
fn losing_the_focus_is_worth_a_redraw() {
    let mut state = State::default();
    state.set_rows(focused_on(strip(), 3));
    assert!(state.live());

    assert!(state.set_rows(focused_on(strip(), 4)));
    assert_eq!(state.selected, 2);
    assert!(!state.live());
}

// -- the history half ---------------------------------------------------

/// The pane is two halves and a rule between them, and the list's share does not depend
/// on what the history happens to have to say - which is the whole reason for the split:
/// a click has to keep landing on the row it points at.
#[test]
fn the_split_is_permanent() {
    let (list_rows, history_rows) = split(20);
    assert_eq!((list_rows, history_rows), (9, 10));
    assert_eq!(list_rows + 1 + history_rows, 20, "the divider is the rest");

    // and it is the same split whether or not there is a history to draw
    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    assert_eq!(split(20).0, list_rows, "no summary, no run");
    state.set_rows(with_summary(strip(), 3, "Ran the tests.", false));
    assert_eq!(split(20).0, list_rows, "a summary");
}

/// The history's share is body and the pinned header, and nothing else: `split` has already
/// taken the divider off the list's, so taking it off again here is what left the last row
/// of the pane blank.
///
/// Nothing observable is printed by a unit test, so the height is read off the room the
/// scroll is clamped against - which is the share less the header's two rows and the rule
/// under them, and nothing else.
#[test]
fn the_history_body_is_its_share_less_the_pinned_header() {
    let mut state = State::default();
    state.set_rows(unfocused(vec![
        StripRow::Project("archive".to_owned()),
        run_row("claude:session-1".to_owned(), "hook history"),
    ]));
    let mut run = a_run();
    // No summary, so the thirty questions are the whole timeline.
    run.summary = None;
    run.entries = (0..30)
        .map(|index| agents::RunEntry {
            tool: None,
            revision: index + 1,
            event_id: None,
            kind: agents::RunEntryKind::Question,
            text: format!("question {index}"),
            created_at_ms: None,
        })
        .collect();
    state.set_run(run);

    let (_, history_rows) = split(19);
    state.render(20, 50);
    assert_eq!(state.last_history_rows, history_rows);
    // Two header rows and the rule under them.
    let body_rows = history_rows - 3;
    assert_eq!(
        state.last_history_max_scroll,
        30 - body_rows,
        "every row of the share the header did not take holds a line of timeline"
    );
    assert_eq!(state.page(), body_rows - 1, "less a line of overlap");

    // A half with no room for the header and a line of timeline both drops the header
    // whole rather than squeezing the timeline: the run's two facts are worth less than
    // what it did.
    let mut cramped = State::default();
    std::mem::swap(&mut cramped, &mut state);
    cramped.render(9, 50);
    let (_, cramped_rows) = split(8);
    assert_eq!(cramped_rows, 4, "three rows of header and one of timeline");
    assert_eq!(
        cramped.last_history_max_scroll,
        30 - cramped_rows,
        "the header is dropped whole on a half with no room for it and a timeline both"
    );
}

/// The offset the host gave for the run's start reaches the row that draws it, and the
/// header is not part of the timeline: scrolling the half moves the entries under it and
/// leaves it where it is.
#[test]
fn the_pinned_header_carries_the_offset_and_does_not_scroll() {
    let mut state = State::default();
    state.set_rows(one_run_selected());
    let mut run = a_run();
    run.summary = None;
    run.session_id = "9f3c1a7e-4b21-4d8e-9c02-1a2b3c4d5e6f".to_owned();
    // 2026-09-04 18:32:07 UTC.
    run.started_at_ms = 1_788_546_727_000;
    run.entries = (0..30)
        .map(|index| agents::RunEntry {
            tool: None,
            revision: index + 1,
            event_id: None,
            kind: agents::RunEntryKind::Question,
            text: format!("question {index}"),
            created_at_ms: None,
        })
        .collect();
    state.set_run(run);
    // What the host would have answered for that instant; a native test build gets `Err`
    // from the shim and falls back to UTC, so it is set here the way `pipe` would.
    state.run_offset_secs = -4 * 3600;

    state.render(20, 60);
    let header: Vec<String> = state
        .history()
        .header
        .iter()
        .map(|line| line.text.clone())
        .collect();
    assert_eq!(
        header,
        vec![
            "started 2026-09-04 14:32:07 -04:00".to_owned(),
            "session 9f3c1a7e-4b21-4d8e-9c02-1a2b3c4d5e6f".to_owned(),
        ]
    );

    assert!(state.scroll_history(5));
    state.render(20, 60);
    let after: Vec<String> = state
        .history()
        .header
        .iter()
        .map(|line| line.text.clone())
        .collect();
    assert_eq!(header, after, "the header is not part of what scrolls");
}

/// With no history half there is no history stop: forward from the list steps straight on
/// to the panel tab bar, and the ring stays four stops only while the pane has four to give.
#[test]
fn a_pane_with_no_history_half_rings_straight_on_to_the_conversation() {
    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    state.render(5, 40);
    assert_eq!(state.last_history_rows, 0);

    assert!(!state.on_key(KeyWithModifier::new(BareKey::Tab)));
    assert_eq!(state.ring_yields, ["tabs"]);
    assert_eq!(
        state.focus,
        Focus::List,
        "and never claimed a half it has not got"
    );
}

/// Backward off the first stop leaves the pane the other way, landing in the box.
#[test]
fn shift_tab_from_the_list_visits_bar_then_history() {
    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    state.render(20, 40);
    assert_eq!(state.focus, Focus::List);

    assert!(state.on_key(KeyWithModifier::new(BareKey::Tab).with_shift_modifier()));
    assert!(state.tabs_focused);
    assert!(state.ring_yields.is_empty());
    state.on_key(KeyWithModifier::new(BareKey::Tab).with_shift_modifier());
    assert!(state.ring_yields.is_empty());
    assert!(!state.tabs_focused);
    assert_eq!(state.focus, Focus::History);
}

/// A terminal that cannot report Shift+Tab sends a plain Tab, which reads as forward. The
/// ring wraps, so nothing is out of reach - backward is a shortcut, never the only way.
#[test]
fn an_unreported_shift_tab_reads_as_a_forward_step() {
    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    state.render(20, 40);

    assert!(state.on_key(KeyWithModifier::new(BareKey::Tab)));
    assert_eq!(
        state.focus,
        Focus::History,
        "forward, as a bare Tab always is"
    );
}

/// The ring arriving from the conversation names the half, and agent has already moved the
/// keyboard: all this does is say where in the pane the arriving keys belong.
#[test]
fn a_take_lands_the_keys_on_the_half_it_names() {
    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    state.render(20, 40);

    assert!(state.fixture_pipe(take(wire::STOP_HISTORY)));
    assert_eq!(state.focus, Focus::History);
    assert!(state.fixture_pipe(take(wire::STOP_LIST)));
    assert_eq!(state.focus, Focus::List);
    assert!(
        !state.fixture_pipe(take(wire::STOP_LIST)),
        "already there, so nothing to redraw"
    );
}

/// The label, the rule and the scroll readout share the divider's row, and the readout is
/// the only thing in the pane that says how far back the history is wound. A kind long
/// enough to fill the row is cut for it rather than pushing it off the end.
#[test]
fn a_long_label_does_not_push_the_scroll_readout_off_the_divider() {
    let kind = "a-very-long-agent-kind-indeed-xx";
    let label = format!("7 · {kind} !");
    for cols in [22, 26, 40, 80] {
        let line = divider_line(&label, 12, 34, cols);
        assert!(line.width() <= cols, "{cols}: {line}");
        assert!(line.ends_with(" ↑12 ↓34 "), "{cols}: {line}");
    }

    // and a divider with nothing wound back has no readout to make room for
    assert_eq!(divider_line("", 0, 0, 6), "──────");
}

/// A pane with room for one half is the list. Two halves of nothing is worse than one
/// list, and the focus has nowhere else to be.
#[test]
fn a_pane_too_short_to_split_is_all_list() {
    assert_eq!(split(6), (6, 0), "a list of three is not worth halving");
    assert_eq!(split(7), (3, 3), "just enough for both");

    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    state.render(5, 40);
    assert_eq!(state.last_history_rows, 0);
    assert!(
        !state.on_key(KeyWithModifier::new(BareKey::Tab)),
        "no other half here, so the keys carry on to the panel tab bar"
    );
    assert_eq!(state.focus, Focus::List);
}

/// The list emptying takes the history half with it, so the keys cannot stay in one: a
/// stop that is not drawn would swallow a Tab moving to an invisible half.
#[test]
fn an_empty_list_puts_the_keys_back_in_the_list_half() {
    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    state.render(20, 40);
    state.on_key(KeyWithModifier::new(BareKey::Tab));
    assert_eq!(state.focus, Focus::History);

    // The last agent exits, and the strip arrives with nothing in it.
    state.set_rows(unfocused(vec![]));
    state.render(20, 40);

    assert_eq!(state.last_history_rows, 0);
    assert_eq!(state.focus, Focus::List, "and the focus came back with it");
    assert!(
        state.on_key(KeyWithModifier::new(BareKey::Tab).with_shift_modifier()),
        "so backward reaches the tab bar"
    );
    assert!(state.tabs_focused);
    assert!(state.ring_yields.is_empty());
}

/// The history is about the row under the cursor, and the divider names it: the pane id
/// and the kind, which is why `StripRow::Agent` carries the kind at all.
#[test]
fn the_history_names_the_selected_agent() {
    let mut state = State::default();
    state.set_rows(with_summary(strip(), 3, "Ran the tests, all green.", false));
    let history = state.history_lines(38);
    assert_eq!(history.label, "3 · claude");
    assert!(
        history
            .entries
            .iter()
            .any(|row| row.text.contains("all green")),
        "{history:?}"
    );

    // and it follows the cursor off that row
    state.on_key(KeyWithModifier::new(BareKey::Char('j')));
    assert!(state.history_lines(38).is_empty(), "pane 7 has no summary");
}

/// `s` is taken on both kinds of row that name an agent, and on neither kind that does
/// not. What it sends is not asserted here - the message reaches the host - but which
/// rows take the key is this pane's whole share of the decision.
#[test]
fn s_is_taken_on_a_row_that_names_an_agent_and_nowhere_else() {
    let mut state = State::default();
    state.set_rows(unfocused(strip()));

    state.selected = 0;
    assert!(
        !state.on_key(KeyWithModifier::new(BareKey::Char('s'))),
        "a project header names no agent"
    );
    assert!(state.summarizing.is_none());

    state.selected = 2;
    assert!(
        state.on_key(KeyWithModifier::new(BareKey::Char('s'))),
        "a live agent does, and the divider says so"
    );
    assert_eq!(asked(&state), Some("pane:3"));
    assert_eq!(state.selected, 2, "and the cursor stays where it was");

    state.set_rows(unfocused(vec![
        StripRow::Project("archive".to_owned()),
        run_row("claude:session-1".to_owned(), "hook history"),
        StripRow::MoreRuns,
    ]));
    state.selected = 1;
    assert!(
        state.on_key(KeyWithModifier::new(BareKey::Char('s'))),
        "an archived run is summarised from what it recorded"
    );
    assert_eq!(asked(&state), Some("run:claude:session-1"));

    state.selected = 2;
    state.summarizing = None;
    assert!(
        !state.on_key(KeyWithModifier::new(BareKey::Char('s'))),
        "`load older runs…` is Enter's, and names nothing to describe"
    );
    assert!(state.summarizing.is_none());
}

/// One archive row, for the resume tests: the shape agent pushes for a completed run.
fn archive() -> Vec<StripRow> {
    vec![
        StripRow::Project("archive".to_owned()),
        run_row("claude:session-1".to_owned(), "hook history"),
        StripRow::MoreRuns,
    ]
}

/// `r` is the one key here that starts a process, so it is taken on exactly the rows that
/// name something resumable and nowhere else. A live agent is deliberately not one of
/// them: `Enter` already walks to its pane, and a second process on its session id would
/// be two agents writing one conversation.
#[test]
fn r_arms_a_confirmation_on_an_archived_run_and_nowhere_else() {
    let mut state = State::default();
    state.set_rows(unfocused(strip()));

    state.selected = 0;
    assert!(!state.on_key(KeyWithModifier::new(BareKey::Char('r'))));
    assert!(state.confirming_resume.is_none(), "a project header");

    state.selected = 2;
    assert!(!state.on_key(KeyWithModifier::new(BareKey::Char('r'))));
    assert!(
        state.confirming_resume.is_none(),
        "a live agent has its pane"
    );

    state.set_rows(unfocused(archive()));
    state.selected = 2;
    assert!(!state.on_key(KeyWithModifier::new(BareKey::Char('r'))));
    assert!(state.confirming_resume.is_none(), "`load older runs…`");

    state.selected = 1;
    assert!(
        state.on_key(KeyWithModifier::new(BareKey::Char('r'))),
        "an archived run does, and the divider says so"
    );
    assert_eq!(
        state.confirming_resume.as_deref(),
        Some("run:claude:session-1")
    );
    assert!(
        state.resume_requests.is_empty(),
        "and nothing has been started yet - that is the whole point of asking"
    );
}

/// Leaving the tab drops the resume question armed there, as it drops a Todos one: left
/// armed, it answered the first key of the next visit, and a `y` then started a process
/// for a question asked about a list since forgotten.
#[test]
fn a_tab_switch_drops_an_armed_resume() {
    let mut state = State::default();
    state.set_rows(unfocused(archive()));
    state.selected = 1;
    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('r'))));
    assert!(state.confirming_resume.is_some());
    state.update(Event::Visible(false));
    state.update(Event::Visible(true));
    assert!(state.confirming_resume.is_none());
}

/// While the search box has the keys, a click only points. Taken as the Enter, a click on
/// the row already chosen walked out of the panel, and two on an event opened the reader
/// over the box - drawing the reader's keys while every key went on into the hidden query.
#[test]
fn a_click_while_the_search_box_is_open_only_points() {
    let mut state = State::default();
    state.set_rows(with_summary(strip(), 3, "Ran the tests.", false));
    state.render(20, 40);
    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('/'))));
    state.render(20, 40);
    let row = (1 + state.last_label_rows + state.selected - state.last_window_start) as isize;
    for _ in 0..2 {
        state.on_mouse(Mouse::LeftClick((row) - 1, 0));
        state.on_mouse(Mouse::Release((row) - 1, 0));
    }
    assert!(state.activations.is_empty());
    assert_eq!(state.panel_hides, 0);
    let event = (1 + state.last_label_rows + state.last_list_rows + 1 + state.last_history_body_top)
        as isize;
    for _ in 0..2 {
        state.on_mouse(Mouse::LeftClick((event) - 1, 0));
        state.on_mouse(Mouse::Release((event) - 1, 0));
    }
    assert_eq!(state.focus, Focus::History, "pointed at");
    assert!(state.reading.is_none());
    assert!(state.search_editing, "and the box still has the keys");
}

/// Tab out of a search box that never searched steps into the history of the agent the
/// cursor was on: leaving the box used to put the cursor back at the top of the list, and
/// the Tab stepped into the first agent's history instead.
#[test]
fn tab_out_of_an_unapplied_search_keeps_the_row() {
    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    state.render(20, 40);
    state.selected = 5;
    assert_eq!(state.selection(), Some(9));
    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('/'))));
    for ch in "ab".chars() {
        state.on_key(KeyWithModifier::new(BareKey::Char(ch)));
    }
    assert!(state.on_key(KeyWithModifier::new(BareKey::Tab)));
    assert!(!state.search_editing);
    assert_eq!(state.selection(), Some(9));
    assert_eq!(state.focus, Focus::History);
}

/// `r` is a letter, so a modified one belongs to whatever the modifiers are bound to.
/// `Ctrl-r` in particular is already this pane's refresh.
#[test]
fn a_modified_r_is_left_alone() {
    let mut state = State::default();
    state.set_rows(unfocused(archive()));
    state.selected = 1;
    assert!(!state.on_key(KeyWithModifier::new(BareKey::Char('r')).with_ctrl_modifier()));
    assert!(state.confirming_resume.is_none());
}

/// `y` is the only key that starts it. Everything else drops the question, and the two
/// that mean "no" stop there rather than also doing whatever they normally do.
#[test]
fn only_y_answers_the_resume_confirmation() {
    let arm = |key: KeyWithModifier| {
        let mut state = State::default();
        state.set_rows(unfocused(archive()));
        state.selected = 1;
        state.on_key(KeyWithModifier::new(BareKey::Char('r')));
        state.on_key(key);
        state
    };

    let state = arm(KeyWithModifier::new(BareKey::Char('y')));
    assert_eq!(state.resume_requests, ["claude:session-1"]);
    assert!(state.confirming_resume.is_none(), "asked and answered");

    for key in [
        KeyWithModifier::new(BareKey::Char('n')),
        KeyWithModifier::new(BareKey::Esc),
        KeyWithModifier::new(BareKey::Char('Y')).with_shift_modifier(),
    ] {
        let state = arm(key.clone());
        assert!(state.resume_requests.is_empty(), "{key} started something");
        assert!(
            state.confirming_resume.is_none(),
            "{key} left the question up"
        );
    }
}

/// `Esc` at the question is one step back, not the way out: the panel stays up, exactly
/// as it does for an `Esc` that closes the reader.
#[test]
fn esc_at_the_question_drops_it_rather_than_the_panel() {
    let mut state = State::default();
    state.set_rows(unfocused(archive()));
    state.selected = 1;
    state.on_key(KeyWithModifier::new(BareKey::Char('r')));
    assert!(
        state.on_key(KeyWithModifier::new(BareKey::Esc)),
        "the divider changed, so the pane redraws"
    );
    assert!(state.confirming_resume.is_none());

    // and a second Esc, with nothing to take back, is the dismissal it always was
    assert!(!state.on_key(KeyWithModifier::new(BareKey::Esc)));
}

/// A key that is neither yes nor no drops the question and then means what it always
/// means. Someone who typed `r` by mistake and carried on walking the list should not
/// have to acknowledge it first - and the cursor moving is itself what ends the question,
/// so the two cannot disagree about which row is being asked about.
#[test]
fn walking_off_the_question_drops_it_and_still_walks() {
    let mut state = State::default();
    state.set_rows(unfocused(archive()));
    state.selected = 1;
    state.on_key(KeyWithModifier::new(BareKey::Char('r')));

    assert!(state.on_key(KeyWithModifier::new(BareKey::Down)));
    assert!(state.confirming_resume.is_none());
    assert_eq!(state.selected, 2, "and the arrow moved");
    assert!(state.resume_requests.is_empty());

    // A `y` now is a `y` at no question at all, and starts nothing.
    state.selected = 1;
    state.on_key(KeyWithModifier::new(BareKey::Char('y')));
    assert!(state.resume_requests.is_empty());
}

/// A strip arriving between the question and the answer can move the cursor. The
/// confirmation names the row it was armed on, so a `y` cannot start a run the user was
/// not looking at when they were asked.
#[test]
fn a_y_after_the_cursor_moved_under_it_starts_nothing() {
    let mut state = State::default();
    state.set_rows(unfocused(archive()));
    state.selected = 1;
    state.on_key(KeyWithModifier::new(BareKey::Char('r')));

    // Same shape, different run: the archive gained a newer row above this one.
    let mut rows = archive();
    rows.insert(1, run_row("claude:session-2".to_owned(), "newer"));
    state.rows = rows;

    state.on_key(KeyWithModifier::new(BareKey::Char('y')));
    assert!(
        state.resume_requests.is_empty(),
        "the row under the cursor is not the row that was asked about"
    );
}

/// The divider carries the question, and then whatever came back. A refusal is shown
/// because the user answered a confirmation: a `y` that silently does nothing is worse
/// than never having been asked. Like every other hint here it ends when the cursor
/// walks off the row it is about.
#[test]
fn the_divider_asks_and_then_says_why_it_was_refused() {
    let mut state = State::default();
    state.set_rows(unfocused(archive()));
    state.selected = 1;
    state.fill_history_cache(48);
    assert_eq!(state.history_label(), "");

    state.on_key(KeyWithModifier::new(BareKey::Char('r')));
    assert_eq!(state.history_label(), "resume? y/n");

    state.on_key(KeyWithModifier::new(BareKey::Char('y')));
    assert_eq!(state.history_label(), "", "the question has been answered");

    let refuse = |state: &mut State, run_id: &str, detail: &str| {
        let result = wire::ResumeResult {
            run_id: run_id.to_owned(),
            detail: detail.to_owned(),
        };
        state.fixture_pipe(PipeMessage::new(
            PipeSource::Plugin(yinz_tile::prelude::PluginId::new(0)),
            "agent_resume_result",
            &Some(serde_json::to_string(&result).expect("encoded")),
            &None,
            false,
        ))
    };

    assert!(refuse(
        &mut state,
        "claude:session-1",
        "this run recorded no session id to resume"
    ));
    assert_eq!(
        state.history_label(),
        "resume failed: this run recorded no session id to resume"
    );
    assert_eq!(
        state.panel_hides, 0,
        "a refusal leaves the keyboard here, and the panel up to say why"
    );

    // An answer about a row the cursor is not on says nothing about the row it is on.
    state.on_key(KeyWithModifier::new(BareKey::Down));
    state.fill_history_cache(48);
    assert_eq!(state.history_label(), "", "walking off ends it");
    assert!(!refuse(&mut state, "claude:session-1", "gone"));
    assert_eq!(state.history_label(), "");
}

/// Asking again clears the last refusal: the note is about a question that has just been
/// asked a second time, and leaving it up beside the new one would read as an answer to
/// it.
#[test]
fn asking_again_clears_the_last_refusal() {
    let mut state = State::default();
    state.set_rows(unfocused(archive()));
    state.selected = 1;
    state.resume_note = Some((
        "run:claude:session-1".to_owned(),
        "the pane could not be opened".to_owned(),
    ));
    state.fill_history_cache(48);
    assert!(state.history_label().starts_with("resume failed"));

    state.on_key(KeyWithModifier::new(BareKey::Char('r')));
    assert!(state.resume_note.is_none());
    assert_eq!(state.history_label(), "resume? y/n");
}

/// A refusal takes the whole divider rather than sharing it with the label: it is the
/// longest thing this row ever says, `fit_label` cuts from the tail, and the half that
/// would be lost is the half that says what went wrong.
///
/// And it comes down for the next request the row makes, `s` as much as `r`. It outranks
/// `summarising…` and outlives it, so leaving it up would make `s` on a row that has just
/// refused an `r` look like a key that does nothing at all.
#[test]
fn a_refusal_takes_the_whole_divider_until_the_row_asks_for_something_else() {
    let mut state = State::default();
    state.set_rows(unfocused(archive()));
    state.selected = 1;
    state.fixture_pipe(PipeMessage::new(
        PipeSource::Plugin(yinz_tile::prelude::PluginId::new(0)),
        "agent_run_detail",
        &Some(serde_json::to_string(&a_run()).expect("encoded")),
        &None,
        false,
    ));
    state.fill_history_cache(48);
    let label = state.history_label();
    assert!(!label.is_empty(), "the row names the run it is about");

    state.resume_note = Some((
        "run:claude:session-1".to_owned(),
        "agent cannot open a pane under /tmp: the plugin mounts claim it".to_owned(),
    ));
    assert_eq!(
        state.history_label(),
        "resume failed: agent cannot open a pane under /tmp: the plugin mounts claim it"
    );

    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('s'))));
    assert!(state.resume_note.is_none());
    assert_eq!(state.history_label(), format!("{label} · summarising…"));
}

/// `s` is a letter, so it must not be taken from whatever the modifiers are bound to,
/// and it must not be mistaken for the history scroll `j`/`k` do.
#[test]
fn a_modified_s_is_left_alone() {
    let mut state = State::default();
    state.set_rows(unfocused(strip()));

    assert!(
        !state.on_key(KeyWithModifier::new(BareKey::Char('s')).with_ctrl_modifier()),
        "ctrl-s is flow control in a great many terminals and is nobody's here"
    );
    assert!(state.summarizing.is_none());
}

/// The hint on the divider is a hint: it says a request is out, and it goes when a summary
/// made after the press arrives, or the cursor moves off the row it was about. A run that
/// merely changes is not an answer - a working agent adds a message while the summary is
/// still being written - and a hint that ended on one read as "done" when nothing had come.
#[test]
fn the_divider_says_a_summary_was_asked_for_until_something_answers() {
    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    state.selected = 2;
    state.fill_history_cache(48);
    assert_eq!(state.history_label(), "");

    state.on_key(KeyWithModifier::new(BareKey::Char('s')));
    assert_eq!(state.history_label(), "summarising…");

    let send = |state: &mut State, run: &agents::AgentRun| {
        state.fixture_pipe(PipeMessage::new(
            PipeSource::Plugin(yinz_tile::prelude::PluginId::new(0)),
            "agent_run_detail",
            &Some(serde_json::to_string(run).expect("encoded")),
            &None,
            false,
        ));
        state.fill_history_cache(48);
    };
    let mut run = a_run();
    run.pane_id = 3;
    run.ended_at_ms = None;
    run.outcome = agents::RunOutcome::Active;
    send(&mut state, &run);
    assert_eq!(
        state.history_label(),
        "claude:session-1 · summarising…",
        "the run moved on, and said nothing about the summary"
    );

    // A summary made after the press is the answer.
    run.entries.push(agents::RunEntry {
        tool: None,
        revision: 5,
        event_id: None,
        kind: agents::RunEntryKind::Summary,
        text: "Tests pass.".to_owned(),
        created_at_ms: Some(now_ms() + 1_000),
    });
    send(&mut state, &run);
    assert_eq!(state.history_label(), "claude:session-1");

    // And walking away from an answer that never came takes the hint with it. `update`
    // is what clears it, and reaches the host to re-send the selection, so what is
    // driven here is the guard that makes the clearing safe rather than the clearing:
    // a hint left standing by any route still cannot claim the row it is not about.
    state.on_key(KeyWithModifier::new(BareKey::Char('s')));
    assert_eq!(state.history_label(), "claude:session-1 · summarising…");
    state.on_key(KeyWithModifier::new(BareKey::Down));
    state.fill_history_cache(48);
    assert_eq!(
        state.history_label(),
        "",
        "the cursor is on another agent now, and that one was not asked about"
    );
}

/// A pane with no hooks is summarised from its screen, and its answer comes in the strip
/// rather than in a run: a summary there stamped after the press ends the hint too, and one
/// from before it does not.
#[test]
fn a_summary_in_the_strip_answers_an_s_on_a_live_agent() {
    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    state.selected = 2;
    state.on_key(KeyWithModifier::new(BareKey::Char('s')));
    let push = |state: &mut State, created_at_ms: Option<i64>| {
        let mut strip = with_summary(strip(), 3, "Ran the tests.", false);
        strip.summaries.get_mut(&3).expect("pushed").created_at_ms = created_at_ms;
        state.fixture_pipe(PipeMessage::new(
            PipeSource::Plugin(yinz_tile::prelude::PluginId::new(0)),
            "agent_strip",
            &Some(serde_json::to_string(&strip).expect("encoded")),
            &None,
            false,
        ))
    };
    push(&mut state, Some(1));
    assert_eq!(
        asked(&state),
        Some("pane:3"),
        "an old summary is not the answer"
    );
    assert!(push(&mut state, Some(now_ms() + 1_000)));
    assert_eq!(asked(&state), None);
}

/// Nothing tells this pane an `s` was refused, so the hint cannot wait for ever: once it
/// has waited longer than the daemon ever takes it says nothing came, and `s` asks again.
#[test]
fn a_summary_nobody_answers_says_so_and_s_asks_again() {
    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    state.selected = 2;
    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('s'))));
    let asked_at = state.summarizing.as_ref().expect("asked").asked_at_ms;
    assert!(!state.expire_summary_ask(asked_at + SUMMARY_WAIT_MS - 1));
    state.fill_history_cache(48);
    assert_eq!(state.history_label(), "summarising…");

    assert!(state.expire_summary_ask(asked_at + SUMMARY_WAIT_MS));
    assert!(
        !state.expire_summary_ask(asked_at + SUMMARY_WAIT_MS + 1),
        "said once"
    );
    assert_eq!(state.history_label(), "no new summary · s to try again");

    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('s'))));
    assert_eq!(state.history_label(), "summarising…");
    assert!(!state.summarizing.as_ref().expect("asked again").stale);
}

/// Every note that takes the divider's place is held whole under it, as a refused resume
/// is: a narrow divider cut "no new summary · s to try again" before the key that asks
/// again.
#[test]
fn a_summary_nobody_wrote_is_wrapped_whole_under_the_divider() {
    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    state.selected = 2;
    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('s'))));
    let asked_at = state.summarizing.as_ref().expect("asked").asked_at_ms;
    assert!(state.expire_summary_ask(asked_at + SUMMARY_WAIT_MS));
    for cols in 22..=40 {
        let lines = state.refusal_lines(cols);
        assert!(
            lines.iter().all(|line| line.width() <= cols - 2),
            "{lines:?} at {cols}"
        );
        assert_eq!(lines.join(" "), "no new summary · s to try again", "{cols}");
    }

    state.render(20, 22);
    let lines = state.refusal_lines(22);
    let header = state.header_rows(state.last_history_rows);
    let held = lines.len().min(state.last_history_rows - header - 1);
    assert!(held > 0);
    assert_eq!(state.last_history_body_top, header + held);
    assert_eq!(
        state.last_history_body_rows,
        state.last_history_rows - header - held
    );

    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('s'))));
    assert!(
        state.refusal_lines(22).is_empty(),
        "asked again, and waiting"
    );
}

#[test]
fn an_archived_run_draws_its_summary_and_timeline() {
    let mut state = State::default();
    state.set_rows(unfocused(vec![
        StripRow::Project("archive".to_owned()),
        run_row("claude:session-1".to_owned(), "hook history"),
    ]));
    state.set_run(a_run());

    let history = state.history_lines(48);
    assert_eq!(history.label, "claude:session-1");
    assert!(history
        .entries
        .iter()
        .any(|row| row.text.contains("summary: History")));
    assert!(history
        .entries
        .iter()
        .any(|row| row.text.contains("question: Keep")));
    assert!(!history
        .entries
        .iter()
        .any(|row| row.text.contains("Bash cargo")));
    assert!(history
        .entries
        .iter()
        .any(|row| row.text.contains("result: Implemented")));
    assert!(!state.on_key(KeyWithModifier::new(BareKey::Enter)));
}

/// Whole segments or none: at every width the hint is the first few of `expected`'s keys,
/// perhaps with the `? keys` marker after them, and only a first key too wide for the row
/// on its own is ever cut. At full width it is `expected` exactly.
///
/// And the marker is there wherever keys were left out and the ones that lead
/// (`agents_hints`) still fit beside it - except in the search box, where `?` is typed
/// rather than opening anything, and which never has one.
fn check_footer(state: &mut State, rows: usize, expected: &str) {
    let segments: Vec<&str> = expected.split(" · ").collect();
    for cols in 1..=expected.width() + 1 {
        state.render(usize::saturating_sub(rows, 1), cols);
        let hint = state.footer_hint().unwrap();
        assert!(hint.width() <= cols, "{hint:?} in {cols} columns");
        assert!(!hint.contains('\n'));
        assert_eq!(state.last_footer_row, Some(rows - 2));
        let keys = hint.strip_suffix(" · ? keys").unwrap_or(&hint);
        let whole = (1..=segments.len()).any(|n| keys == segments[..n].join(" · "));
        let cut = segments[0].width() > cols && segments[0].starts_with(keys);
        assert!(whole || cut, "{hint:?} in {cols} columns");
        if cols >= expected.width() {
            assert_eq!(hint, expected);
        }
        if state.typing() {
            assert!(!hint.ends_with("? keys"), "{hint:?} in {cols} columns");
            continue;
        }
        let keep = state.agents_hints().1.clamp(1, segments.len());
        let leads = segments[..keep].join(" · ").width() + " · ? keys".width();
        if keys != expected && leads <= cols {
            assert!(hint.ends_with(" · ? keys"), "{hint:?} in {cols} columns");
        }
    }
}

#[test]
fn footer_hints_follow_the_view_and_fit_one_line() {
    let mut state = State::default();
    // Nothing to act on: no row keys, no history half to Tab into - nor a conversation to
    // Tab to, as far as anybody has said, until agent sends its first strip.
    check_footer(&mut state, 12, "/ search · Esc hide");
    state.strip_seen = true;
    check_footer(&mut state, 12, "/ search · Tab bar · Esc hide");
    state.set_rows(unfocused(strip()));
    check_footer(
        &mut state,
        12,
        "Enter focus · s summary · p previous · w waiting (0) · / search · Tab history",
    );
    assert!(state.begin_predecessors());
    check_footer(
        &mut state,
        12,
        "Esc/c clear · Enter focus · s summary · p previous · w waiting (0) · / search · \
         Tab history",
    );
    assert!(state.begin_search());
    check_footer(&mut state, 12, "Enter apply · Esc cancel");
    state.search_query = "a query".into();
    check_footer(&mut state, 12, "Enter apply · Esc cancel · Ctrl-u clear");
    assert!(state.search_key(KeyWithModifier::new(BareKey::Enter)));
    check_footer(&mut state, 12, "searching… · Esc cancel");
    let result = search_reply(&mut state, &[]);
    assert!(state.apply_search_result(result));
    check_footer(&mut state, 12, "Esc/c clear · / edit");
    state.search_truncated = true;
    check_footer(&mut state, 12, "Esc/c clear · / edit");
    // A failure keeps the box open, and says why under it - so the footer is the keys.
    assert!(state.begin_search());
    assert!(state.search_key(KeyWithModifier::new(BareKey::Enter)));
    let mut failed = wire::SearchResult {
        request_id: state.search_request.to_string(),
        ..Default::default()
    };
    failed.error = "store busy".into();
    assert!(state.apply_search_result(failed));
    check_footer(&mut state, 12, "Enter retry · Esc back");
    assert!(state.clear_search());
    check_footer(
        &mut state,
        12,
        "Enter focus · s summary · p previous · w waiting (0) · / search · Tab history",
    );
}

/// The footer is read off the row under the cursor as well as the view: keys that work
/// there, and none that do not - no `p` on a run, no `s` on `load older runs…`, no `Enter`
/// on an empty list - with a question's answer keys first, and somebody waiting ahead of
/// the row's own keys.
#[test]
fn footer_hints_name_the_keys_for_the_row_under_the_cursor() {
    let mut state = State::default();
    let mut rows = strip();
    rows.extend(archive());
    state.set_rows(unfocused(rows));
    state.render(20, 120);
    let footer = |state: &mut State| {
        state.render(20, 200);
        state.footer_hint().unwrap()
    };
    state.selected = 7;
    assert_eq!(
        footer(&mut state),
        "Enter history · r resume · s summary · w waiting (0) · / search · Tab history"
    );
    state.on_key(KeyWithModifier::new(BareKey::Char('r')));
    assert_eq!(footer(&mut state), "y resume · n cancel");
    state.on_key(KeyWithModifier::new(BareKey::Char('n')));
    state.resume_note = Some(("run:claude:session-1".into(), "no session id".into()));
    assert!(footer(&mut state).starts_with("resume failed: no session id · Enter history"));
    state.resume_note = None;

    state.selected = 8;
    assert_eq!(
        footer(&mut state),
        "Enter load older runs · w waiting (0) · / search · Tab history"
    );

    // The history half on a run: its events, then the run's own keys.
    state.selected = 7;
    state.set_run(a_run());
    state.focus = Focus::History;
    assert_eq!(
        footer(&mut state),
        "↑↓ event · Enter read · y copy · r resume · s summary · w waiting (0) · / search · \
         PgUp/PgDn scroll · Tab bar"
    );
    // Unless another pane has the keyboard, when the half does not claim them.
    state.has_keyboard = Some(false);
    assert!(footer(&mut state).starts_with("Enter history"));
    state.has_keyboard = None;
    state.focus = Focus::List;

    // Somebody waiting goes ahead of the row's keys.
    let mut waiting = strip();
    waiting[3] = StripRow::Agent {
        pane_id: 7,
        text: "  7  codex  asking".into(),
        kind: "codex".into(),
        waiting: true,
    };
    state.set_rows(unfocused(waiting));
    state.selected = 2;
    assert_eq!(
        footer(&mut state),
        "w waiting (1) · Enter focus · s summary · p previous · / search · Tab history"
    );
    assert!(state.toggle_waiting());
    assert_eq!(
        footer(&mut state),
        "Esc/w all · 1 waiting · Enter focus · s summary · p previous · / search · \
         Tab history"
    );
    assert!(state.clear_waiting());

    // The reader's own keys, by what kind of reader it is.
    let mut state = reading(&task_notification());
    let reader = |state: &mut State| {
        state.render(20, 120);
        state.footer_hint().unwrap()
    };
    // The keys nothing on screen suggests come before the `Enter` the fold glyphs do.
    assert_eq!(
        reader(&mut state),
        "Esc back · j/k fold · y copy · v source · Enter open · PgUp/PgDn page"
    );
    for _ in 0..3 {
        press(&mut state, BareKey::Char('j'));
    }
    assert_eq!(
        reader(&mut state),
        "Esc back · j/k fold · y copy · o file · v source · Enter open · PgUp/PgDn page",
        "on the fold that names a file"
    );
    press(&mut state, BareKey::Char('v'));
    assert_eq!(
        reader(&mut state),
        "Esc back · ↑↓ scroll · y copy · o file · v folds · PgUp/PgDn page"
    );
}

/// At the widths a panel is drawn the keys fill the footer to within a few columns, which
/// left `? keys` no room at all. The footer gives up a trailing key or two to say it - but
/// never the row's verb, nor anything ahead of it - and the search box, where `?` is typed,
/// never says it.
#[test]
fn the_footer_names_the_key_sheet_at_the_default_width() {
    let footer = |state: &mut State, cols: usize| {
        state.render(11, cols);
        state.render(11, cols);
        state.footer_hint().unwrap()
    };
    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    assert_eq!(footer(&mut state, 24), "Enter focus · ? keys");
    assert_eq!(footer(&mut state, 34), "Enter focus · s summary · ? keys");

    let mut state = State::default();
    state.set_rows(unfocused(archive()));
    state.selected = 1;
    assert_eq!(footer(&mut state, 24), "Enter history · ? keys");

    let mut state = reading(&task_notification());
    assert_eq!(footer(&mut state, 24), "Esc back · ? keys");

    // The history half's verb and the key before it take the row with no room left over:
    // the marker gives way to them.
    let mut state = State::default();
    state.set_rows(with_summary(strip(), 3, "Ran the tests.", false));
    state.focus = Focus::History;
    assert_eq!(footer(&mut state, 24), "↑↓ event · Enter read");

    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    state.begin_search();
    for query in ["", "a query"] {
        state.search_query = query.into();
        for cols in 1..=45 {
            let hint = footer(&mut state, cols);
            assert!(!hint.contains("? keys"), "{hint:?} at {cols}");
        }
    }
}

#[test]
fn the_footer_leaves_room_for_the_header_and_content_on_resize() {
    let mut state = State::default();
    state.set_rows(unfocused((0..20).map(|id| agent(id, "claude")).collect()));
    for cols in [0, 1, 40] {
        for (rows, list, history, footer) in [
            (12, 4, 5, Some(11)),
            (0, 0, 0, None),
            (1, 0, 0, None),
            (2, 1, 0, None),
            (3, 1, 0, Some(2)),
            (4, 2, 0, Some(3)),
            (8, 6, 0, Some(7)),
            (9, 3, 3, Some(8)),
            (12, 4, 5, Some(11)),
        ] {
            state.render(usize::saturating_sub(rows, 1), cols);
            let expected = if cols == 0 {
                (0, 0, None)
            } else {
                (list, history, footer.map(|row| row - 1))
            };
            assert_eq!(
                (
                    state.last_list_rows,
                    state.last_history_rows,
                    state.last_footer_row,
                ),
                expected,
                "{rows} rows, {cols} columns"
            );
            assert_eq!(state.footer_hint().is_some(), expected.2.is_some());
        }
    }
}

#[test]
fn the_footer_ignores_clicks_and_wheel_events() {
    let mut state = reading(&"word ".repeat(1000));
    state.close_reader();
    state.rows.extend((0..20).map(|id| agent(id, "claude")));
    for rows in [3, 12] {
        state.render(rows, 50);
        state.list_scroll = Some(3);
        state.history_scroll = 1;
        for focus in [Focus::List, Focus::History] {
            state.focus = focus;
            state.tabs_focused = true;
            state.bar_focused = vec![state.own_client_id];
            let selected = state.selected;
            let footer = state.last_footer_row.unwrap() as isize;
            for event in [
                Mouse::Hover(footer, 49),
                Mouse::ScrollUp(1),
                Mouse::ScrollDown(1),
                Mouse::LeftClick(footer, 0),
                Mouse::RightClick(footer, 0),
                Mouse::ScrollUp(1),
                Mouse::ScrollDown(1),
            ] {
                assert!(!state.on_mouse(event));
                assert_eq!(state.selected, selected);
                assert_eq!(state.list_scroll, Some(3));
                assert_eq!(state.history_scroll, 1);
                assert_eq!(state.focus, focus);
                assert!(state.tabs_focused);
            }
        }
    }
}

/// The reader has a footer of its own, with its own keys - not the list's, which would be
/// wrong over it - and the other tabs still have none: their hints are on a row of their
/// own already. The footer's row is not the reader's, so the wheel over it does nothing.
#[test]
fn readers_have_their_own_footer_and_other_tabs_none() {
    let mut state = reading(&"word ".repeat(1000));
    state.render(11, 50);
    assert_eq!(
        state.footer_hint().as_deref(),
        Some("Esc back · ↑↓ scroll · y copy · PgUp/PgDn page")
    );
    assert_eq!(state.last_footer_row, Some(10));
    assert_eq!(state.last_reader_rows, 9);
    state.on_mouse(Mouse::Hover((11) - 1, 0));
    assert!(!state.on_mouse(Mouse::ScrollDown(1)), "the footer's row");
    state.on_mouse(Mouse::Hover((10) - 1, 0));
    assert!(state.on_mouse(Mouse::ScrollDown(1)));
    state.close_reader();
    state.render(11, 50);
    assert_eq!(state.last_footer_row, Some(10));
}

/// The Agents footer shows tab-bar keys over the list, reader, and an open search box
/// while the bar has keyboard focus. It restores the view's hints when focus leaves.
#[test]
fn the_bar_hint_is_the_footer_wherever_the_bar_has_the_keys() {
    let mut state = reading(&"word ".repeat(1000));
    for reader in [true, false] {
        if !reader {
            state.close_reader();
            state.set_rows(unfocused(strip()));
        }
        state.tabs_focused = true;
        state.bar_focused = vec![state.own_client_id];
        state.render(11, 60);
        assert_eq!(state.footer_hint().as_deref(), Some(tabs::BAR_HINT));
        assert_eq!(state.last_footer_row, Some(10));

        // Another pane took the keyboard: the stop is kept, but nothing claims it.
        state.has_keyboard = Some(false);
        state.render(11, 60);
        assert!(state.tabs_focused);
        assert_ne!(state.footer_hint().as_deref(), Some(tabs::BAR_HINT));
        state.has_keyboard = None;

        state.tabs_focused = false;

        state.bar_focused = vec![];
        state.render(11, 60);
        assert_eq!(state.last_footer_row, Some(10));
        assert_ne!(state.footer_hint().as_deref(), Some(tabs::BAR_HINT));
    }

    // The bar owns keys and hints even when a search box remains open.
    state.begin_search();
    state.tabs_focused = true;
    state.bar_focused = vec![state.own_client_id];
    state.render(11, 60);
    assert_eq!(state.footer_hint().as_deref(), Some(tabs::BAR_HINT));
}

/// Ctrl-o moves the panel from wherever the keys are in it, including every state that
/// answers all the other keys itself - and changes nothing inside it on the way.
#[test]
fn ctrl_o_is_taken_ahead_of_every_state_that_answers_every_key() {
    let ctrl_o = || KeyWithModifier::new(BareKey::Char('o')).with_ctrl_modifier();
    let mut state = reading("An event being read");
    assert!(!state.on_key(ctrl_o()));
    assert!(state.reading.is_some(), "the reader stays open");

    let mut state = State::default();
    let mut rows = strip();
    rows.extend(archive());
    state.set_rows(unfocused(rows));
    state.begin_search();
    state.search_query = "typed".into();
    assert!(!state.on_key(ctrl_o()));
    assert!(state.search_editing);
    assert_eq!(state.search_query, "typed", "nothing typed into the box");
    state.clear_search();

    state.tabs_focused = true;

    state.bar_focused = vec![state.own_client_id];
    assert!(state.on_key(ctrl_o()));
    assert!(state.tabs_focused, "the bar consumes modified keys too");
    state.tabs_focused = false;
    state.bar_focused = vec![];

    state.selected = 7;
    assert!(state.arm_resume());
    assert!(!state.on_key(ctrl_o()));
    assert_eq!(
        state.confirming_resume.as_deref(),
        Some("run:claude:session-1"),
        "the question stays armed and on screen"
    );
}

#[test]
fn native_bar_focus_uses_the_sending_client() {
    let mut state = State {
        own_client_id: ClientId::new(1),
        bar_focused: vec![ClientId::new(1)],
        ..Default::default()
    };
    let shift_tab = KeyWithModifier::new(BareKey::Tab).with_shift_modifier();
    state.update(Event::Key(shift_tab.clone()));
    assert!(state.ring_yields.is_empty());
    assert!(state.bar_focused.is_empty());

    state.bar_focused.push(ClientId::new(1));
    state.own_client_id = ClientId::new(2);
    state.update(Event::Key(shift_tab));
    assert!(state.ring_yields.is_empty());
    assert_eq!(state.bar_focused, [ClientId::new(1), ClientId::new(2)]);

    state.pane_tabs = Some(PaneTabsId(7));
    state.update(Event::PaneTabsUpdate(vec![]));
    assert_eq!(state.pane_tabs, None);
    assert!(state.bar_focused.is_empty());
}

#[test]
fn queued_bar_keys_do_not_reach_the_list() {
    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    state.render(20, 40);
    state.selected = 3;
    state.take_ring_stop(Some(agent_proto::tabs::STOP_TABS));
    for key in [
        BareKey::Left,
        BareKey::Right,
        BareKey::Down,
        BareKey::Char('j'),
        BareKey::Char('k'),
        BareKey::Char('s'),
        BareKey::Char('x'),
    ] {
        assert!(state.on_key(KeyWithModifier::new(key)));
        assert!(state.tabs_focused);
        assert_eq!(state.selected, 3);
        assert_eq!(asked(&state), None);
    }
    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('r')).with_ctrl_modifier()));
    assert!(state.tabs_focused);
    assert!(!state.on_paste("queued paste"));
    for release in [BareKey::Enter, BareKey::Tab, BareKey::Esc] {
        state.take_ring_stop(Some(agent_proto::tabs::STOP_TABS));
        assert!(state.on_key(KeyWithModifier::new(release)));
        assert!(!state.tabs_focused);
        assert_eq!(state.selected, 3);
    }
}

/// A click takes the keys off the bar wherever it lands, and that is a redraw even on a
/// row nothing can be chosen on.
#[test]
fn a_click_off_the_bar_is_a_redraw_wherever_it_lands() {
    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    state.render(20, 40);
    state.focus = Focus::List;
    state.tabs_focused = true;
    state.bar_focused = vec![state.own_client_id];
    assert!(
        state.on_mouse(Mouse::LeftClick((1) - 1, 0)),
        "the project header"
    );
    assert!(!state.tabs_focused);
    assert_eq!(state.selected, 2);
}

/// A click that brings the keys back - off the tab bar, off another stop of the ring, or
/// into this pane from another - only points, even on the row already chosen: the click
/// after it, with the keys here, is the Enter. Walking the user out of the panel on the
/// click that was only meant to get the keys back made the mouse a trap.
#[test]
fn a_click_bringing_the_keys_back_only_points() {
    for away in ["shift-tab", "take", "another pane"] {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        state.render(20, 40);
        match away {
            "shift-tab" => {
                assert!(state.on_key(KeyWithModifier::new(BareKey::Tab).with_shift_modifier()));
                assert!(state.tabs_focused);
            },
            "take" => {
                state.take_ring_stop(Some(agent_proto::tabs::STOP_TABS));
                assert!(state.tabs_focused);
            },
            _ => state.has_keyboard = Some(false),
        }
        // The selected row, pane 3, under the tab bar.
        let row = 1 + state.selected as isize;
        state.on_mouse(Mouse::LeftClick((row) - 1, 0));
        assert!(state.activations.is_empty(), "{away}");
        assert_eq!(state.panel_hides, 0, "{away}");
        assert!(state.keys_here() || away == "another pane", "{away}");
        // What the manifest the click makes the host send says.
        state.has_keyboard = Some(true);
        state.on_mouse(Mouse::LeftClick((row) - 1, 0));
        assert!(state.activations.is_empty(), "{away}: not on the press");
        state.on_mouse(Mouse::Release((row) - 1, 0));
        assert_eq!(state.activations, [3], "{away}");
    }

    // In the history half too: the click that brings the keys back to the event under the
    // cursor points at it, and the next one reads it.
    let mut state = State::default();
    state.set_rows(with_summary(strip(), 3, "Ran the tests.", false));
    state.render(20, 40);
    state.focus = Focus::History;
    state.has_keyboard = Some(false);
    let event = (state.last_list_rows + 2) as isize;
    state.on_mouse(Mouse::LeftClick((event) - 1, 0));
    assert!(state.reading.is_none());
    state.has_keyboard = Some(true);
    state.on_mouse(Mouse::LeftClick((event) - 1, 0));
    assert!(state.reading.is_none(), "not on the press");
    assert!(state.on_mouse(Mouse::Release((event) - 1, 0)));
    assert!(state.reading.is_some(), "the idle summary is an event too");
}

/// Esc on the bar is a step back to the list, the way Esc on the conversation's bar is a
/// step back to its box - not the whole panel put away.
#[test]
fn esc_on_the_tab_bar_returns_to_the_list() {
    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    state.render(20, 40);
    state.focus = Focus::History;
    state.tabs_focused = true;
    state.bar_focused = vec![state.own_client_id];
    assert!(state.on_key(KeyWithModifier::new(BareKey::Esc)));
    assert!(!state.tabs_focused);
    assert_eq!(state.focus, Focus::List);
    assert_eq!(state.panel_hides, 0);
}

/// The tab bar is the panel's only map, so a filtered Agents view does not take it away:
/// the two tabs stay drawn and clickable on every surface, and the view's name is the
/// Agents surface's own first row, with the list a row below it.
#[test]
fn the_tab_bar_stays_up_in_every_view_and_the_view_names_itself_above_the_list() {
    for view in ["search", "typing", "previous", "waiting"] {
        let mut state = State::default();
        let mut rows = strip();
        for (index, pane_id) in [(2, 3), (3, 7)] {
            rows[index] = StripRow::Agent {
                pane_id,
                text: format!("  {pane_id}  claude  asking"),
                kind: "claude".into(),
                waiting: true,
            };
        }
        state.set_rows(unfocused(rows));
        let label = match view {
            "search" => {
                let result = search_reply(&mut state, &["claude:a", "claude:b"]);
                assert!(state.apply_search_result(result));
                "Search: history"
            },
            "typing" => {
                state.begin_search();
                state.search_query = "half".into();
                "Search: half"
            },
            "previous" => {
                state.selected = 2;
                assert!(state.begin_predecessors());
                "Previous agents:   3  claude  asking"
            },
            _ => {
                assert!(state.toggle_waiting());
                "Waiting for you"
            },
        };
        state.render(11, 40);
        assert_eq!(state.view_label().as_deref(), Some(label), "{view}");
        assert_eq!(state.last_label_rows, 1);
        // Row 1 is the label, and the list starts on row 2.
        assert!(!state.on_mouse(Mouse::LeftClick((1) - 1, 0)), "{view}");
        let first = state.rows.iter().position(selectable).unwrap();
        let last = state.rows.iter().rposition(selectable).unwrap();
        assert_ne!(first, last, "{view}");
        state.selected = last;
        assert!(state.on_mouse(Mouse::LeftClick((2 + first as isize) - 1, 0)));
        assert_eq!(state.selected, first, "{view}");
    }
}

/// A paste lands in the box being typed in, as the typed characters would have, and
/// nowhere else: on the list it would have been a string of single-letter commands.
#[test]
fn a_paste_extends_a_search_being_typed_and_nothing_else() {
    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    let before = state.selected;
    assert!(!state.update(Event::PastedText("sjp/w".into())));
    assert_eq!(state.selected, before);
    assert!(state.summarizing.is_none());
    assert!(state.waiting_base.is_none() && state.predecessors.is_none());

    state.begin_search();
    state.search_query = "build".into();
    assert!(state.search_key(KeyWithModifier::new(BareKey::Enter)));
    assert!(state.search_pending);
    let request = state.search_request;
    assert!(state.update(Event::PastedText(" failing\r\n  on macOS\n".into())));
    assert_eq!(state.search_query, "build failing on macOS");
    assert!(
        !state.search_pending,
        "the reply in flight was for another query"
    );
    assert_ne!(state.search_request, request);
    assert!(!state.update(Event::PastedText("\n\n".into())));
}

#[test]
fn a_paste_is_one_line_with_no_control_characters() {
    assert_eq!(single_line("one\ntwo\r\nthree\rfour"), "one two three four");
    assert_eq!(single_line("  padded  \n\n\n  lines "), "  padded lines ");
    assert_eq!(
        single_line(" one line, as it was "),
        " one line, as it was "
    );
    assert_eq!(single_line("copied line\n"), "copied line");
    assert_eq!(single_line("tab\there\u{1b}[31m"), "tab here [31m");
    assert_eq!(single_line("\n \n"), "");
}

/// Putting the panel away puts the keyboard stop back on the list and drops every question
/// left armed, whichever half did the hiding. What the user was looking at stays.
#[test]
fn a_hide_ends_the_visit_but_keeps_the_view() {
    let pane = |shown: bool, focused: bool| {
        Event::PaneUpdate(PaneManifest {
            panes: std::collections::HashMap::from([(
                TabPosition::new(0),
                vec![PaneInfo {
                    id: PaneId::Plugin(PluginId::default()),
                    is_suppressed: !shown,
                    is_focused: focused,
                    ..Default::default()
                }],
            )]),
        })
    };
    let mut rows = strip();
    rows.extend(archive());
    for hide in ["esc", "manifest"] {
        let mut state = State::default();
        state.set_rows(unfocused(rows.clone()));
        state.render(20, 40);
        state.update(pane(true, true));
        state.selected = 7;
        assert!(state.arm_resume());
        state.focus = Focus::History;
        state.tabs_focused = true;
        state.bar_focused = vec![state.own_client_id];
        match hide {
            "esc" => state.hide_the_panel(),
            _ => assert!(!state.update(pane(false, false)), "nothing shown to redraw"),
        }
        assert!(!state.tabs_focused, "{hide}");
        assert_eq!(state.focus, Focus::List, "{hide}");
        assert!(state.confirming_resume.is_none(), "{hide}");
        assert_eq!(state.selected, 7, "the selection is kept");
        state.update(pane(true, true));
        state.on_key(KeyWithModifier::new(BareKey::Char('y')));
        assert!(
            state.resume_requests.is_empty(),
            "{hide}: y answers nothing now"
        );
    }

    // A surface other than Agents is kept too, and so is a filtered view.
    let mut state = State::default();
    state.set_rows(unfocused(rows.clone()));
    state.toggle_waiting();
    state.tabs_focused = true;
    state.bar_focused = vec![state.own_client_id];
    state.update(pane(true, true));
    state.update(pane(false, false));
    assert!(state.waiting_base.is_some());
    assert!(!state.tabs_focused);
}

/// The keyboard leaving this pane only changes what is drawn - the stops stay where they
/// are, for the keys to come back to - and the first manifest is no edge at all.
#[test]
fn losing_the_keyboard_redraws_but_keeps_the_stop() {
    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    assert!(
        state.keys_here(),
        "told nothing, the pane assumes the keys are here"
    );
    assert!(
        !state.own_pane_changed(true, false),
        "the first manifest without the focus does not redraw"
    );
    assert!(!state.keys_here());
    assert!(state.own_pane_changed(true, true));
    assert!(state.keys_here());
    assert!(!state.own_pane_changed(true, true), "no edge, no redraw");
    state.focus = Focus::History;
    assert!(state.own_pane_changed(true, false));
    assert_eq!(
        state.focus,
        Focus::History,
        "the stop is kept for the way back"
    );
    assert!(!state.keys_here());
    assert!(
        !state.own_pane_changed(false, false),
        "a hidden pane has nothing on screen to correct"
    );
    assert_eq!(state.focus, Focus::List, "but hiding it ends the visit");
}

#[test]
fn the_wheel_scrolls_the_list_without_selecting_and_clicks_use_its_window() {
    let mut state = State::default();
    state.set_rows(unfocused((0..20).map(|id| agent(id, "claude")).collect()));
    state.render(11, 50);
    let selection = state.selection_key();
    state.history_scroll = 2;
    state.focus = Focus::History;
    assert!(!state.on_mouse(Mouse::Hover((1) - 1, 0)));
    assert!(state.on_mouse(Mouse::ScrollDown(3)));
    state.on_mouse(Mouse::Hover((state.last_list_rows as isize) - 1, 49));
    assert!(
        state.on_mouse(Mouse::ScrollDown(3)),
        "wheel events can precede a render"
    );
    state.render(11, 50);
    assert_eq!(state.last_window_start, 6);
    assert_eq!(state.selection_key(), selection);
    assert_eq!(state.focus, Focus::History);
    // render clamps the empty history; the wheel itself must leave it alone.
    state.history_scroll = 2;
    assert!(state.on_mouse(Mouse::ScrollDown(100)));
    assert_eq!(state.history_scroll, 2);
    assert!(!state.on_mouse(Mouse::ScrollDown(1)));
    assert!(state.on_mouse(Mouse::ScrollUp(1)));
    state.render(11, 50);
    assert_eq!(state.last_window_start, 15);
    assert!(state.on_mouse(Mouse::LeftClick((2) - 1, 0)));
    assert_eq!(state.selected, 16);
    // The window stays where it is, so the row clicked stays under the pointer for a second
    // click; a key hands it back to the selection.
    assert_eq!(state.list_scroll, Some(15));
    state.render(11, 50);
    assert_eq!(state.last_window_start, 15);
    assert!(state.on_key(KeyWithModifier::new(BareKey::Down)));
    assert_eq!(state.list_scroll, None);
}

/// In a list longer than its window, the first click of a double click used to re-centre
/// the list on the row, so the second one landed on another row and walked into nothing.
#[test]
fn a_double_click_walks_into_the_agent_anywhere_in_a_long_list() {
    let mut state = State::default();
    state.set_rows(unfocused((0..20).map(|id| agent(id, "claude")).collect()));
    state.selected = 10;
    state.render(11, 50);
    let start = state.last_window_start;
    assert!(start > 0 && start < 10, "a window centred on the cursor");
    // The window's first row, under the tab bar: not the cursor's, and not the centre.
    let line = 1;
    state.on_mouse(Mouse::LeftClick((line) - 1, 0));
    state.on_mouse(Mouse::Release((line) - 1, 0));
    assert_eq!(state.selected, start, "the first click points");
    assert!(state.activations.is_empty());
    state.render(11, 50);
    assert_eq!(
        state.last_window_start, start,
        "the row stayed under the pointer"
    );
    state.on_mouse(Mouse::LeftClick((line) - 1, 0));
    assert!(state.activations.is_empty(), "not on the press");
    state.on_mouse(Mouse::Release((line) - 1, 0));
    assert_eq!(state.activations, [start as u32]);
}

#[test]
fn manual_list_scroll_survives_refreshes_and_clamps_to_the_available_rows() {
    let rows: Vec<_> = (0..20).map(|id| agent(id, "claude")).collect();
    let mut state = State::default();
    state.set_rows(unfocused(rows.clone()));
    state.render(11, 50);
    // Without a pointer position, the wheel uses keyboard focus.
    assert!(state.on_mouse(Mouse::ScrollDown(100)));
    assert!(!state.set_rows(unfocused(rows.clone())));
    state.render(15, 50);
    assert_eq!(state.last_window_start, 14);
    state.set_rows(unfocused(rows[..10].to_vec()));
    state.render(15, 50);
    assert_eq!(state.last_window_start, 4);
    assert_eq!(state.selected, 0);
    assert!(
        state.on_key(KeyWithModifier::new(BareKey::Up)),
        "reveal even at the first agent"
    );
    state.render(15, 50);
    assert_eq!(state.last_window_start, 0);
    assert!(state.on_mouse(Mouse::ScrollDown(1)));
    state.set_rows(focused_on(rows.clone(), 9));
    state.render(15, 50);
    assert_eq!(state.list_scroll, None);
    assert_eq!(state.selected, 9);
    assert_eq!(state.last_window_start, 6);

    state.set_rows(unfocused(Vec::new()));
    state.render(15, 50);
    assert!(!state.on_mouse(Mouse::ScrollDown(1)));
    state.set_rows(unfocused(rows));
    state.render(1, 50);
    assert_eq!(state.last_history_rows, 0);
    assert!(state.on_mouse(Mouse::ScrollDown(1)));
    state.render(0, 50);
    assert_eq!(state.last_list_rows, 0);
    assert!(!state.on_mouse(Mouse::ScrollDown(1)));
    state.render(0, 50);
    assert!(!state.on_mouse(Mouse::ScrollDown(1)));
    state.render(11, 50);
    state.on_mouse(Mouse::Hover((-1) - 1, 0));
    assert!(
        !state.on_mouse(Mouse::ScrollDown(1)),
        "pointer left the pane"
    );
    state.on_mouse(Mouse::Hover((12) - 1, 0));
    assert!(
        !state.on_mouse(Mouse::ScrollDown(1)),
        "outside the rendered pane"
    );
}

#[test]
fn header_and_empty_panes_preserve_list_history_and_reader_positions() {
    for reader in [false, true] {
        let mut state = reading(&"word ".repeat(1000));
        state.rows.extend((0..20).map(|id| agent(id, "claude")));
        state.list_scroll = Some(3);
        state.history_scroll = 1;
        if reader {
            assert!(state.scroll_reader(3));
        } else {
            state.close_reader();
        }
        state.render(11, 50);
        let position = |state: &State| {
            (
                state.selected,
                state.list_scroll,
                state.history_scroll,
                state.history_cursor,
                state.reading.as_ref().map(|r| (r.scroll, r.cursor)),
                state.focus,
                state.tabs_focused,
            )
        };
        let before = position(&state);
        let dimensions = |state: &State| {
            [
                state.last_window_start,
                state.last_list_rows,
                state.last_history_rows,
                state.last_history_body_rows,
                state.last_history_max_scroll,
                state.last_reader_rows,
                state.last_reader_max_scroll,
            ]
        };
        let visible = dimensions(&state);
        for (line, column) in [(0, 0), (-1, 0), (12, 0), (1, 50)] {
            state.on_mouse(Mouse::Hover((line) - 1, column));
            assert!(!state.on_mouse(Mouse::ScrollUp(1)));
            assert!(!state.on_mouse(Mouse::ScrollDown(1)));
            if line != 0 {
                assert!(!state.on_mouse(Mouse::LeftClick((line) - 1, column)));
            }
            assert_eq!(position(&state), before);
        }
        for (rows, cols) in [(1, 50), (0, 50), (12, 0)] {
            state.render(usize::saturating_sub(rows, 1), cols);
            assert_eq!(
                (state.last_rows, state.last_cols),
                (usize::saturating_sub(rows, 1), cols)
            );
            assert_eq!(dimensions(&state), [0; 7]);
            state.mouse_position = None;
            assert!(!state.on_mouse(Mouse::ScrollUp(1)));
            assert!(!state.on_mouse(Mouse::ScrollDown(1)));
            state.on_mouse(Mouse::Hover((1) - 1, 0));
            assert!(!state.on_mouse(Mouse::ScrollUp(1)));
            assert!(!state.on_mouse(Mouse::ScrollDown(1)));
            assert!(!state.on_mouse(Mouse::LeftClick((1) - 1, 0)));
            assert_eq!(position(&state), before);

            state.render(11, 50);
            assert_eq!(position(&state), before);
            // Only the active surface's geometry is restored.
            let restored = dimensions(&state);
            if reader {
                assert_eq!(restored[5..], visible[5..]);
            } else {
                assert_eq!(restored[..5], visible[..5]);
            }
            state.on_mouse(Mouse::Hover((1) - 1, 0));
            assert!(state.on_mouse(Mouse::ScrollDown(1)));
            assert!(state.on_mouse(Mouse::ScrollUp(1)));
            assert_eq!(position(&state), before);
        }
    }
}

#[test]
fn the_wheel_scrolls_folded_and_raw_readers_without_touching_the_list() {
    for raw in [false, true] {
        let mut state = reading(&long_notification());
        if raw {
            press(&mut state, BareKey::Char('v'));
        } else {
            for _ in 0..5 {
                press(&mut state, BareKey::Char('j'));
            }
            press(&mut state, BareKey::Enter);
        }
        state.on_mouse(Mouse::ScrollUp(1000));
        state.render(19, 50);
        let selected = state.selected;
        let history_scroll = state.history_scroll;
        state.on_mouse(Mouse::Hover((1) - 1, 0));
        assert!(state.on_mouse(Mouse::ScrollDown(1)));
        assert_eq!(state.reading.as_ref().unwrap().scroll, 1);
        assert!(state.on_mouse(Mouse::ScrollDown(1000)));
        assert!(!state.on_mouse(Mouse::ScrollDown(1)));
        assert!(state.on_mouse(Mouse::ScrollUp(1)));
        assert_eq!(
            state.reading.as_ref().unwrap().scroll,
            state.last_reader_max_scroll - 1
        );
        assert_eq!(state.selected, selected);
        assert_eq!(state.history_scroll, history_scroll);
        assert_eq!(state.list_scroll, None);
    }
}

/// The wheel over history stops at both ends. Unclamped, a wheel at the top would count
/// coming back down would cost as many notches as were wasted going up.
#[test]
fn the_wheel_scrolls_the_history_and_stops_at_both_ends() {
    let mut state = State::default();
    state.set_rows(unfocused(vec![
        StripRow::Project("archive".to_owned()),
        run_row("claude:session-1".to_owned(), "hook history"),
    ]));
    let mut run = a_run();
    run.entries.push(agents::RunEntry {
        tool: None,
        revision: 5,
        event_id: None,
        kind: agents::RunEntryKind::Question,
        text: "Ship it?".to_owned(),
        created_at_ms: None,
    });
    state.set_run(run);
    // Four rows below the divider, against five timeline rows including the result.
    state.render(9, 50);
    assert_eq!(state.last_history_rows, 4);
    assert_eq!(state.last_history_max_scroll, 1);

    assert!(!state.on_mouse(Mouse::Hover(((state.last_list_rows + 1) as isize) - 1, 0)));
    assert!(state.on_mouse(Mouse::ScrollUp(1)));
    assert_eq!(state.history_scroll, 1);
    assert!(
        !state.on_mouse(Mouse::ScrollUp(50)),
        "already at the oldest line, and nothing to redraw"
    );
    assert_eq!(state.history_scroll, 1, "the clamp, not the count");
    assert!(state.on_mouse(Mouse::ScrollDown(1)), "one notch back down");
    assert_eq!(state.history_scroll, 0);
    assert!(!state.on_mouse(Mouse::ScrollDown(1)));
    assert_eq!(
        state.focus,
        Focus::List,
        "scrolling history does not take the keys"
    );
    assert_eq!(state.list_scroll, None);
    // The last rendered row and the first row below the divider belong to history too.
    state.on_mouse(Mouse::Hover((8) - 1, 0));
    assert!(state.on_mouse(Mouse::ScrollUp(1)));
    state.on_mouse(Mouse::Hover(((state.last_list_rows + 2) as isize) - 1, 0));
    assert!(state.on_mouse(Mouse::ScrollDown(1)));
    state.mouse_position = None;
    state.focus = Focus::History;
    assert!(
        state.on_mouse(Mouse::ScrollUp(1)),
        "focus is the fallback without coordinates"
    );
}

/// The history half keeps its parse of the run across resizes and across the run growing
/// (`LoadedRun::parsed`), and neither may change what it draws: after both, the half is exactly
/// what the run drawn from scratch at the new width is.
#[test]
fn a_resized_half_over_a_grown_run_draws_what_a_fresh_one_does() {
    let mut state = State::default();
    state.set_rows(unfocused(vec![
        StripRow::Project("archive".to_owned()),
        run_row("claude:session-1".to_owned(), "hook history"),
    ]));
    let mut run = a_run();
    // Unstamped, so the ages cannot tick over between the two draws being compared.
    for entry in &mut run.entries {
        entry.created_at_ms = None;
    }
    state.set_run(run.clone());
    state.render(13, 60);
    state.render(13, 41);

    run.entries.push(agents::RunEntry {
        tool: None,
        revision: 9,
        event_id: None,
        kind: agents::RunEntryKind::Question,
        text: "Shall I **push** it?".to_owned(),
        created_at_ms: None,
    });
    state.set_run(run.clone());
    state.render(13, 41);

    let fresh = agents::run_history(&run, 41, now_ms(), state.run_offset_secs);
    assert_eq!(state.cached_history_lines(41), &fresh);
    assert!(fresh.entries.iter().any(|line| line.text.contains("push")));
}

/// The run's parse is touched only when a run is loaded - a resize step draws what is
/// held, and so does a strip push, which moves `generation` on any pane's change and says
/// nothing about this run. Read by poisoning the held parse with a shorter copy of the run,
/// as `the_reader_is_wrapped_once_per_event_fold_and_width` poisons its cache.
#[test]
fn the_runs_parse_is_only_brought_up_to_date_by_a_loaded_run() {
    let mut state = State::default();
    state.set_rows(one_run_selected());
    let mut run = a_run();
    for entry in &mut run.entries {
        entry.created_at_ms = None;
    }
    state.set_run(run.clone());
    state.render(13, 60);

    let mut poison = run.clone();
    poison.entries.truncate(1);
    state.run.as_mut().expect("a run").parsed = agents::ParsedRun::new(&poison);
    let fresh = |run: &agents::AgentRun, offset| agents::run_history(run, 41, now_ms(), offset);
    let offset = state.run_offset_secs;
    assert_eq!(
        state.cached_history_lines(41),
        &fresh(&poison, offset),
        "a resize re-wraps what is held"
    );

    state.generation += 1;
    assert_eq!(
        state.cached_history_lines(41),
        &fresh(&poison, offset),
        "a strip push leaves the run's timeline alone"
    );

    state.set_run(run.clone());
    assert_eq!(
        state.cached_history_lines(41),
        &fresh(&run, offset),
        "a loaded run brings it up to date"
    );
}

/// A cursor that leaves the run's row and comes back finds the parse where it left it: the
/// run has not changed, and parsing a long one again is seconds. Poisoned as above, so a
/// parse made again would show.
#[test]
fn stepping_off_the_run_and_back_keeps_its_parse() {
    let mut state = State::default();
    state.set_rows(unfocused(vec![
        StripRow::Project("archive".to_owned()),
        run_row("claude:session-1".to_owned(), "hook history"),
        run_row("claude:session-2".to_owned(), "another run"),
    ]));
    state.selected = 1;
    let mut run = a_run();
    for entry in &mut run.entries {
        entry.created_at_ms = None;
    }
    state.set_run(run.clone());
    state.render(13, 60);

    let mut poison = run;
    poison.entries.truncate(1);
    state.run.as_mut().expect("a run").parsed = agents::ParsedRun::new(&poison);
    let offset = state.run_offset_secs;

    state.selected = 2;
    state.render(13, 41);
    assert!(state.run.is_some(), "the run is still held");
    state.selected = 1;
    assert_eq!(
        state.cached_history_lines(41),
        &agents::run_history(&poison, 41, now_ms(), offset),
        "back on the run, the held parse is drawn"
    );
}

/// Only a run that says something new moves `run_revision`, and a run arriving does not
/// move `generation`: agent sends the selected run again each time it is asked, and the
/// strip's counter is not the run's.
#[test]
fn only_a_changed_run_moves_the_run_revision() {
    let mut state = State::default();
    state.set_rows(unfocused(archive()));
    state.selected = 1;
    let generation = state.generation;
    let send = |state: &mut State, run: &agents::AgentRun| {
        state.fixture_pipe(PipeMessage::new(
            PipeSource::Plugin(yinz_tile::prelude::PluginId::new(0)),
            "agent_run_detail",
            &Some(serde_json::to_string(run).expect("encoded")),
            &None,
            false,
        ))
    };
    let mut run = a_run();
    assert!(send(&mut state, &run));
    assert_eq!(state.run_revision, 1);
    assert!(!send(&mut state, &run), "the same run says nothing");
    assert_eq!(state.run_revision, 1);

    run.entries[0].text.push_str(" - and more");
    assert!(send(&mut state, &run));
    assert_eq!(state.run_revision, 2);
    assert_eq!(state.generation, generation);

    // The fingerprint goes with the run, so a cleared one cannot make the same run
    // arriving again look like one already held.
    state.run = None;
    assert!(send(&mut state, &run), "a run cleared is taken again");
    assert_eq!(state.run_revision, 3);
}

/// The summary is a row of the timeline like any other, so a talkative one costs the
/// events beside it nothing they cannot be scrolled back to: the whole half winds over it.
#[test]
fn a_long_summary_scrolls_off_the_top_like_any_other_event() {
    let mut state = State::default();
    state.set_rows(unfocused(vec![
        StripRow::Project("archive".to_owned()),
        run_row("claude:session-1".to_owned(), "hook history"),
    ]));
    let mut run = a_run();
    run.summary = Some(agents::Summary {
        // Four rows of it at this width - more than the half has to give, which under the
        // old pinned block would have left one row of timeline showing whatever the scroll.
        text: "word ".repeat(40),
        ..run.summary.take().unwrap()
    });
    state.set_run(run);

    // Three rows below the divider, and every one of them a line of timeline.
    state.render(8, 50);
    assert_eq!(state.last_history_rows, 3);
    let lines = state.cached_history_lines(50).entries.len();
    assert!(lines > 5, "a summary longer than the half: {lines} lines");
    assert_eq!(state.last_history_max_scroll, lines - 3);

    // At rest the half is against the newest end, and the events above the summary have
    // been wound past - under the old pinned block the top of it was always on screen.
    let lines = state.cached_history_lines(50).entries.len();
    assert!(!state.cached_history_lines(50).entries[lines - 3..]
        .iter()
        .any(|row| row.text.contains("prompt:")),);

    // Wound all the way back, the oldest event is at the top of the half and the tail of
    // the summary has scrolled off the bottom.
    state.on_mouse(Mouse::Hover(((state.last_list_rows + 1) as isize) - 1, 0));
    assert!(state.on_mouse(Mouse::ScrollUp(50)));
    state.render(8, 50);
    assert_eq!(state.history_scroll, state.last_history_max_scroll);
    let history = state.cached_history_lines(50);
    assert!(history.entries[0]
        .text
        .contains("prompt: implement history"));
    assert!(
        !history.entries[..3]
            .iter()
            .any(|row| row.text.ends_with("word") && !row.text.contains("summary:")),
        "only the first line of the summary is still on screen"
    );
}

/// `Tab` and `Shift+Tab` walk the ring, and `j`/`k` and the arrows do whatever the half they
/// are in does with a step - a row of the list, an event of the timeline.
///
/// Directional now: forward off the history half leaves this pane for the conversation, so
/// getting back to the list is `Shift+Tab` rather than another `Tab`.
#[test]
fn tab_walks_the_ring_and_jk_follow_the_focus() {
    let mut state = State::default();
    state.set_rows(unfocused(vec![
        StripRow::Project("archive".to_owned()),
        run_row("claude:session-1".to_owned(), "hook history"),
        run_row("claude:session-2".to_owned(), "another"),
    ]));
    let mut run = a_run();
    run.entries.push(agents::RunEntry {
        tool: None,
        revision: 5,
        event_id: None,
        kind: agents::RunEntryKind::Question,
        text: "Ship it?".to_owned(),
        created_at_ms: None,
    });
    state.set_run(run);
    // Three rows of timeline, with older events available above them.
    state.render(8, 50);
    assert_eq!(state.selected, 1);

    assert!(state.on_key(KeyWithModifier::new(BareKey::Tab)));
    assert_eq!(state.focus, Focus::History);
    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('k'))));
    assert_eq!(
        state.history_cursor, 1,
        "k walks the timeline it is in, an event at a time"
    );
    assert_eq!(state.selected, 1, "and moves no selection");

    // The arrows follow the focus too: in the history half they walk its events, and leave
    // the list and the keys where they are.
    assert!(state.on_key(KeyWithModifier::new(BareKey::Down)));
    assert_eq!(
        state.history_cursor, 0,
        "Down is j, a step toward the newest"
    );
    assert!(state.on_key(KeyWithModifier::new(BareKey::Up)));
    assert_eq!(state.history_cursor, 1, "Up is k");
    assert_eq!((state.selected, state.focus), (1, Focus::History));

    // Shift+Tab is the way back to the list, where they walk the rows.
    assert!(state.on_key(KeyWithModifier::new(BareKey::Tab).with_shift_modifier()));
    assert_eq!(state.focus, Focus::List);
    assert!(state.on_key(KeyWithModifier::new(BareKey::Down)));
    assert_eq!(state.selected, 2);
    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('k'))));
    assert_eq!(state.selected, 1, "k walks the list it is back in");

    assert!(state.on_key(KeyWithModifier::new(BareKey::Tab)));
    assert_eq!(state.focus, Focus::History);
    // Forward off the history half is off the end of this pane: the conversation's tab bar
    // is the next stop, and the keys go with a request rather than a redraw.
    assert!(!state.on_key(KeyWithModifier::new(BareKey::Tab)));
    assert_eq!(state.ring_yields, ["tabs"]);
    assert_eq!(
        state.focus,
        Focus::History,
        "the sub-focus stays where it was left - the pane frame is what says the keys are \
         gone, and coming back by a click should land where the user was"
    );
    // Backward returns from the bar to history, then to the list.
    assert!(state.on_key(KeyWithModifier::new(BareKey::Tab).with_shift_modifier()));
    assert_eq!(state.focus, Focus::History);
    assert!(state.on_key(KeyWithModifier::new(BareKey::Tab).with_shift_modifier()));
    assert_eq!(state.focus, Focus::List);

    // An arrow with nowhere to go is nothing to redraw, in either half.
    state.on_key(KeyWithModifier::new(BareKey::Down));
    assert_eq!(state.selected, 2, "the last row there is");
    assert!(!state.on_key(KeyWithModifier::new(BareKey::Down)));
    state.on_key(KeyWithModifier::new(BareKey::Tab));
    assert_eq!(state.focus, Focus::History);
    state.history_cursor = 0;
    assert!(
        !state.on_key(KeyWithModifier::new(BareKey::Down)),
        "already on the newest event"
    );
    assert_eq!((state.selected, state.focus), (2, Focus::History));
}

/// A strip of one archived run, the shape every history test below wants.
fn one_run_selected() -> Strip {
    unfocused(vec![
        StripRow::Project("archive".to_owned()),
        run_row("claude:session-1".to_owned(), "hook history"),
    ])
}

/// Walking the cursor onto an event the half is not showing winds the half to it. The key
/// handler cannot do this itself: how many lines the half has depends on the width, and
/// only `render` is told the width.
#[test]
fn the_cursor_winds_the_half_back_to_the_event_it_lands_on() {
    let mut state = State::default();
    state.set_rows(one_run_selected());
    let mut run = a_run();
    run.summary = None;
    run.entries = (0..30)
        .map(|index| agents::RunEntry {
            tool: None,
            revision: index + 1,
            event_id: None,
            kind: agents::RunEntryKind::Question,
            text: format!("question {index}"),
            created_at_ms: None,
        })
        .collect();
    state.set_run(run);
    state.focus = Focus::History;
    state.render(20, 50);
    // Thirty one-line events against the six rows the pinned header leaves of the nine:
    // the half is at the newest end.
    assert_eq!(state.last_history_rows, 9);
    assert_eq!(state.last_history_body_rows, 6);
    assert_eq!(state.history_scroll, 0);

    // Walk the cursor past the top of the window and the half follows it back.
    for _ in 0..12 {
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('k'))));
    }
    state.render(20, 50);
    assert_eq!(state.history_cursor, 12);
    assert_eq!(
        state.history_scroll, 7,
        "wound back just far enough to put the event on the oldest row"
    );

    // And back down again: the half winds forward only as far as it has to.
    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('j'))));
    state.render(20, 50);
    assert_eq!(state.history_cursor, 11);
    assert_eq!(state.history_scroll, 7, "still on screen, so nothing moved");

    // The cursor stops at the oldest event rather than running off the end.
    for _ in 0..50 {
        state.on_key(KeyWithModifier::new(BareKey::Char('k')));
    }
    state.render(20, 50);
    assert_eq!(state.history_cursor, 29);
    assert_eq!(state.history_scroll, state.last_history_max_scroll);
    assert!(!state.on_key(KeyWithModifier::new(BareKey::Char('k'))));
}

/// The oldest event is where anybody looking for something older ends up, and the notice
/// that older events were not loaded is the row above it - a row no event owns, which
/// winding the window to the event's own first row left just off the top.
#[test]
fn walking_to_the_oldest_event_shows_the_omitted_notice() {
    let mut state = State::default();
    state.set_rows(one_run_selected());
    let mut run = a_run();
    run.summary = None;
    run.omitted_entries = 3;
    run.entries = (0..30)
        .map(|index| agents::RunEntry {
            tool: None,
            revision: index + 1,
            event_id: None,
            kind: agents::RunEntryKind::Question,
            text: format!("question {index}"),
            created_at_ms: None,
        })
        .collect();
    state.set_run(run);
    state.focus = Focus::History;
    state.render(20, 50);
    for _ in 0..50 {
        state.on_key(KeyWithModifier::new(BareKey::Char('k')));
    }
    state.render(20, 50);
    assert_eq!(state.history_cursor, 29);
    assert_eq!(state.history_scroll, state.last_history_max_scroll);
    assert_eq!(state.last_history_window_start, 0);
    let notice = &state.history().entries[0].text;
    assert!(notice.contains("older events not loaded"), "{notice}");
}

/// `Enter` in the history half opens the event under the cursor, uncapped and with the
/// whole pane to read it in; `Esc` puts the reader away and leaves the pane where it is.
/// On a run, the list half's `Enter` is the way into that half: the run has no pane to walk
/// to, and the two halves have separate cursors.
#[test]
fn enter_in_the_history_half_opens_the_event_and_esc_comes_back() {
    let mut state = State::default();
    state.set_rows(one_run_selected());
    let mut run = a_run();
    run.summary = None;
    run.entries = vec![agents::RunEntry {
        tool: None,
        revision: 1,
        event_id: None,
        kind: agents::RunEntryKind::Prompt,
        text: "word ".repeat(400),
        created_at_ms: Some(1),
    }];
    state.set_run(run);
    state.render(20, 50);

    // Capped in the half, whole in the reader.
    assert_eq!(
        state.cached_history_lines(50).entries.len(),
        agents::MAX_ENTRY_ROWS
    );
    // In the list half a run has no pane to walk into, so Enter walks into its history -
    // and opens nothing yet, and leaves the panel up.
    assert!(state.on_key(KeyWithModifier::new(BareKey::Enter)));
    assert_eq!(state.focus, Focus::History);
    assert!(state.reading.is_none());
    assert_eq!(state.panel_hides, 0);

    assert!(state.on_key(KeyWithModifier::new(BareKey::Enter)));
    let reading = state.reading.as_ref().expect("the event under the cursor");
    assert_eq!(reading.kind, "prompt");
    assert_eq!(reading.created_at_ms, Some(1));
    assert_eq!(&*reading.text, "word ".repeat(400));

    state.render(20, 50);
    assert!(
        state.reader_lines().len() > agents::MAX_ENTRY_ROWS,
        "the whole of it, which is the point of opening it"
    );
    assert!(state.last_reader_max_scroll > 0);

    // The reader has the keys while it is up, and scrolls from its head.
    assert!(!state.on_key(KeyWithModifier::new(BareKey::Char('k'))));
    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('j'))));
    assert_eq!(state.reading.as_ref().unwrap().scroll, 1);
    assert_eq!(state.selected, 1, "and steers nothing behind it");

    assert!(state.on_key(KeyWithModifier::new(BareKey::Esc)));
    assert!(state.reading.is_none(), "Esc closed the reader");
    assert!(state.reader_cache.is_none(), "and let go of its copy");
}

/// A summary somebody asked for arrives as an entry of the run, so the reader's divider
/// says what it is and how old it is - and the minute tick has to keep it honest.
#[test]
fn the_reader_is_an_age_that_goes_on_ticking() {
    let mut state = State::default();
    state.set_rows(one_run_selected());
    let mut run = a_run();
    run.summary = None;
    run.entries = vec![agents::RunEntry {
        tool: None,
        revision: 1,
        event_id: None,
        kind: agents::RunEntryKind::Prompt,
        text: "implement history".to_owned(),
        created_at_ms: None,
    }];
    state.set_run(run);
    state.render(20, 50);
    state.focus = Focus::History;
    state.on_key(KeyWithModifier::new(BareKey::Enter));
    assert!(state.reading.is_some());
    assert!(!state.shows_an_age(), "an unstamped event has none to tick");

    state.reading.as_mut().unwrap().created_at_ms = Some(1);
    assert!(state.shows_an_age());
}

/// The reader is about an event of the row the cursor was on, so walking off that row
/// takes it with you: a strip whose focus lands on another agent moves the selection, and
/// what is on screen has to follow.
#[test]
fn a_strip_that_moves_the_selection_closes_the_reader() {
    let mut state = State::default();
    state.set_rows(with_summary(strip(), 3, "Ran the tests.", false));
    state.render(20, 50);
    state.focus = Focus::History;
    assert!(state.on_key(KeyWithModifier::new(BareKey::Enter)));
    assert!(state.reading.is_some(), "the idle summary is an event too");

    // What `pipe` does with a strip, less the host call it ends with.
    let was = state.selection_key();
    state.set_rows(focused_on(strip(), 7));
    assert!(state.follow_selection(was));
    assert_eq!(state.selection(), Some(7), "the focus walked onto pane 7");
    assert!(state.reading.is_none());
}

/// The reader's body is a markdown parse of the whole event, on a pane that redraws on
/// every keystroke - so it is wrapped once per event, fold and width, the way the history
/// half below it is. Read by poisoning the cache, as `the_history_is_rendered_once_per_thing
/// _it_depends_on` does.
#[test]
fn the_reader_is_wrapped_once_per_event_fold_and_width() {
    let mut state = State::default();
    state.set_rows(one_run_selected());
    let mut run = a_run();
    run.summary = None;
    run.entries = vec![agents::RunEntry {
        tool: None,
        revision: 1,
        event_id: None,
        kind: agents::RunEntryKind::Prompt,
        text: "word ".repeat(50),
        created_at_ms: None,
    }];
    state.set_run(run);
    state.render(20, 50);
    state.focus = Focus::History;
    state.on_key(KeyWithModifier::new(BareKey::Enter));
    state.render(20, 50);

    let poison = envelope::ReaderLines {
        lines: vec![markdown::MdLine::plain("poison")],
        starts: Vec::new(),
    };
    let key = state.reader_cache.as_ref().unwrap().0;
    state.reader_cache = Some((key, poison.clone()));
    state.fill_reader_cache(50);
    assert_eq!(state.reader_lines(), poison.lines, "same event, same width");
    state.fill_reader_cache(40);
    assert_ne!(state.reader_lines(), poison.lines, "a resize rewraps it");

    // And a second opening is a second event, however alike the two look.
    state.on_key(KeyWithModifier::new(BareKey::Esc));
    state.on_key(KeyWithModifier::new(BareKey::Enter));
    state.fill_reader_cache(40);
    assert_ne!(state.reader_lines(), poison.lines);
}

/// The page keys are the history's from either half: the list has no page of its own,
/// following the selection during keyboard navigation.
#[test]
fn the_page_keys_are_the_historys_from_either_half() {
    let mut state = State::default();
    state.set_rows(unfocused(vec![
        StripRow::Project("archive".to_owned()),
        run_row("claude:session-1".to_owned(), "hook history"),
    ]));
    let mut run = a_run();
    run.entries = (0..40)
        .map(|index| agents::RunEntry {
            tool: None,
            revision: index + 1,
            event_id: None,
            kind: agents::RunEntryKind::Question,
            text: format!("question {index}"),
            created_at_ms: None,
        })
        .collect();
    state.set_run(run);
    state.render(20, 50);

    assert_eq!(state.focus, Focus::List, "the page keys did not need it");
    // A frame between the presses, so a window wound back to the cursor on render would
    // show: in the history half the cursor has to come to the window instead.
    for focus in [Focus::List, Focus::History] {
        state.focus = focus;
        assert!(state.on_key(KeyWithModifier::new(BareKey::PageUp)));
        state.render(20, 50);
        assert_eq!(state.history_scroll, state.page(), "{focus:?}");
        assert!(state.on_key(KeyWithModifier::new(BareKey::PageDown)));
        state.render(20, 50);
        assert_eq!(state.history_scroll, 0, "{focus:?}");
        assert!(
            !state.on_key(KeyWithModifier::new(BareKey::PageDown)),
            "at the newest line already"
        );
    }
}

/// A click lands in the half it points at. In the list it moves the cursor and no more;
/// below the divider it points at the event under it, and on the divider the focus is all
/// it does.
#[test]
fn a_click_lands_in_the_half_it_points_at() {
    let mut state = State::default();
    state.set_rows(with_summary(strip(), 3, "Ran the tests.", false));
    state.render(20, 40);
    let (list_rows, history_rows) = split(19);
    assert_eq!(state.last_list_rows, list_rows);
    assert_eq!(state.last_history_rows, history_rows);

    assert!(
        state.on_mouse(Mouse::LeftClick(((list_rows + 1) as isize) - 1, 0)),
        "the divider takes the focus"
    );
    assert_eq!(state.focus, Focus::History);
    assert_eq!(state.selected, 2, "and selects nothing");
    assert!(
        !state.on_mouse(Mouse::LeftClick(((list_rows + 3) as isize) - 1, 0)),
        "already down here, and on no event"
    );
    // A click on the event the cursor is on, with the keys already down here, reads it -
    // once it comes up on the cell it went down on.
    let event = (list_rows + 2) as isize;
    state.on_mouse(Mouse::LeftClick((event) - 1, 0));
    assert!(state.reading.is_none(), "not on the press");
    assert!(state.on_mouse(Mouse::Release((event) - 1, 0)));
    assert!(state.reading.is_some(), "the idle summary is an event too");
    assert!(state.on_key(KeyWithModifier::new(BareKey::Esc)));
    // A press on it that ends as a drag - to copy its text - reads nothing.
    state.on_mouse(Mouse::LeftClick((event) - 1, 0));
    state.on_mouse(Mouse::Hold((event) - 1, 6));
    assert!(!state.on_mouse(Mouse::Release((event) - 1, 6)));
    assert!(state.reading.is_none(), "a drag is not a click");

    assert!(
        state.on_mouse(Mouse::LeftClick((4) - 1, 0)),
        "the list takes it back"
    );
    assert_eq!(state.focus, Focus::List);
    assert_eq!(state.selected, 3, "pane 7, the row that was pointed at");
    assert!(
        state.activations.is_empty(),
        "and pointed at it is all: a click does not walk into the agent"
    );

    // and a click on a header inside the list still hits no row, but the half is the
    // half the pointer is in
    state.focus = Focus::History;
    assert!(
        state.on_mouse(Mouse::LeftClick((1) - 1, 0)),
        "the Project header"
    );
    assert_eq!(state.focus, Focus::List);
    assert_eq!(state.selected, 3, "still pane 7");
}

/// A click is an arrow key with a destination: it moves the cursor, the history half
/// follows it there, and `Enter` is what actually walks into the agent. Clicking used to
/// do both, which meant looking at an agent's events cost you the pane you were in.
#[test]
fn a_click_selects_and_enter_is_what_focuses() {
    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    state.render(20, 40);
    assert_eq!(state.selected, 2, "the first agent, pane 3");

    // wound into the history of the row the cursor starts on, so the reset that
    // belongs to leaving a row has something to undo
    state.history_scroll = 4;
    let was = state.selection_key();

    assert!(state.on_click(3, true), "the row under the pointer");
    assert_eq!(state.selected, 3, "pane 7");
    assert!(
        state.activations.is_empty(),
        "a click moved the cursor and nothing else"
    );
    assert_eq!(state.panel_hides, 0, "and left the panel up");
    // what `update` does after every event, and the whole of how the history half
    // learns which agent it is now about
    assert!(
        state.follow_selection(was),
        "the cursor left the row it was on"
    );
    assert_eq!(state.history_scroll, 0, "the half is about pane 7 now");

    state.on_key(KeyWithModifier::new(BareKey::Enter));
    assert_eq!(state.activations, [7], "and Enter walks into it");
    assert_eq!(
        state.panel_hides, 1,
        "putting the panel away on the way, so the agent is not walked into behind it"
    );

    // A second click on the row the cursor is on, with the keys already in the list, is the
    // Enter - but not the click that brings the keys back from the history half onto it.
    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    state.render(20, 40);
    state.focus = Focus::History;
    assert!(state.on_click(2, true), "back to the list");
    assert!(state.activations.is_empty());
    assert!(
        !state.on_click(2, true),
        "the panel went, so nothing here to redraw"
    );
    assert_eq!(state.activations, [3]);
    assert_eq!(state.panel_hides, 1);

    // `load older runs…` has nothing to preview, so its first click is its Enter.
    let mut rows = strip();
    rows.push(StripRow::MoreRuns);
    let mut state = State::default();
    state.set_rows(unfocused(rows));
    state.render(20, 40);
    assert!(state.on_click(6, true));
    assert_eq!(state.archive_requests, 1);
    assert_eq!(state.panel_hides, 0);
}

/// Only the Enter that actually walks into an agent dismisses the panel. Every other Enter
/// is about something inside the panel, and putting the panel away would take the answer
/// off screen with it.
#[test]
fn only_an_enter_on_an_agent_puts_the_panel_away() {
    // `reading` is the history half's Enter: it opens the event under that half's own
    // cursor, which is a thing drawn in this pane.
    let mut state = reading("implement history");
    assert!(state.reading.is_some(), "the reader is up");
    assert_eq!(
        state.panel_hides, 0,
        "the reader is in this pane - hiding it would be closing what was just opened"
    );
    assert!(state.activations.is_empty(), "and nothing was walked into");

    // Esc backs out of the reader rather than falling through to the dismissal
    press(&mut state, BareKey::Esc);
    assert!(state.reading.is_none(), "back to the list");
    assert_eq!(state.panel_hides, 0, "one step back, not the way out");

    // a project header: Enter is inert on it, and an inert key hides nothing
    let mut state = State::default();
    state.set_rows(with_summary(strip(), 3, "Ran the tests.", false));
    state.render(20, 40);
    state.focus = Focus::List;
    state.selected = 0;
    assert!(
        matches!(state.rows.first(), Some(StripRow::Project { .. })),
        "the strip starts on a header"
    );
    state.on_key(KeyWithModifier::new(BareKey::Enter));
    assert_eq!(
        state.panel_hides, 0,
        "Enter on a header does nothing at all"
    );

    // `load older runs…` asks agent for more of the archive, in this pane
    let mut rows = strip();
    rows.push(StripRow::MoreRuns);
    state.set_rows(unfocused(rows));
    state.selected = state.rows.len() - 1;
    assert!(matches!(
        state.rows.get(state.selected),
        Some(StripRow::MoreRuns)
    ));
    state.on_key(KeyWithModifier::new(BareKey::Enter));
    assert_eq!(
        state.panel_hides, 0,
        "the rows it asks for arrive here, so the list has to still be on screen"
    );
    assert!(state.activations.is_empty(), "and it is not an agent");

    // an archived run walks into its history, in this pane
    let mut rows = strip();
    rows.extend(archive());
    state.set_rows(unfocused(rows));
    state.render(20, 40);
    state.selected = 7;
    assert!(state.on_key(KeyWithModifier::new(BareKey::Enter)));
    assert_eq!(state.focus, Focus::History);
    assert_eq!(state.panel_hides, 0, "what Enter found is in the panel");
    assert!(state.activations.is_empty());
    state.focus = Focus::List;

    // and the one that is
    state.selected = 2;
    assert!(matches!(
        state.rows.get(state.selected),
        Some(StripRow::Agent { .. })
    ));
    state.on_key(KeyWithModifier::new(BareKey::Enter));
    assert_eq!(state.activations, [3], "the first agent");
    assert_eq!(state.panel_hides, 1, "and this Enter is the one that hides");
}

/// The list windows above the divider, so a click maps to the row under the pointer
/// rather than the one the history is covering.
#[test]
fn the_list_windows_above_the_divider() {
    let mut state = State::default();
    // a strip longer than the window, so the rows the history covers are real rows
    let mut rows = strip();
    for pane_id in 20..30 {
        rows.push(agent(pane_id, &format!("  {pane_id}  claude  idle")));
    }
    state.set_rows(with_summary(rows, 3, "Ran the tests, all green.", false));

    let (list_rows, history_rows) = split(12);
    assert_eq!((list_rows, history_rows), (5, 6));
    assert!(
        state.rows.len() > list_rows,
        "and the strip outruns what is left"
    );
    assert_eq!(
        click_target(&state.rows, 0, list_rows, list_rows),
        None,
        "the divider is not a row"
    );
    assert_eq!(
        click_target(&state.rows, 1, list_rows, list_rows - 1),
        Some(5),
        "the last row of the list is still clickable"
    );
}

/// `render_divider` colours the `!` in a label by *searching* the line for the character
/// — the label is cut to fit, so its position cannot be counted. That makes every glyph
/// the label can carry a hazard: one containing a `!` would light up the wrong column.
/// The hook marker is the newest of them, so it is worth pinning here rather than
/// trusting that nobody ever picks an exclamation mark for one.
#[test]
fn the_hook_marker_cannot_be_mistaken_for_the_waiting_one() {
    assert!(!agents::HOOK_MARKER.contains('!'));

    let mut state = State::default();
    state.set_rows(with_summary(
        vec![
            StripRow::Project("agent".to_owned()),
            StripRow::Agent {
                pane_id: 3,
                text: "  3  claude◆  waiting".to_owned(),
                kind: "claude◆".to_owned(),
                waiting: true,
            },
        ],
        3,
        "Waiting on a permission prompt.",
        true,
    ));
    let label = state.history_lines(38).label;
    // The one `!` in the label is the waiting marker, and it is the last of it rather
    // than anywhere inside the kind.
    assert!(label.contains("3 · claude◆"), "{label}");
    assert_eq!(label.matches('!').count(), 1, "{label}");
    assert!(
        label.find('!').unwrap() > label.find('◆').unwrap(),
        "{label}"
    );
}

/// Rendering the half is a markdown parse per entry, and the half is redrawn on every
/// keystroke, so it is computed once per thing it depends on rather than once per frame.
///
/// Read by poisoning the cache: a second render that draws the poison never recomputed,
/// which is the only observable difference between a hit and a very fast miss.
#[test]
fn the_history_is_rendered_once_per_thing_it_depends_on() {
    let mut state = State::default();
    state.set_rows(unfocused(vec![
        StripRow::Project("archive".to_owned()),
        run_row("claude:session-1".to_owned(), "hook history"),
    ]));
    state.set_run(a_run());
    state.render(20, 50);

    let poison = agents::HistoryLines {
        label: "poison".to_owned(),
        ..agents::HistoryLines::default()
    };
    let key = state.history_cache.as_ref().unwrap().0.clone();
    state.history_cache = Some((key, poison.clone()));

    assert_eq!(
        state.cached_history_lines(50),
        &poison,
        "same row, same width, same run - nothing to parse again"
    );
    assert_ne!(
        state.cached_history_lines(40),
        &poison,
        "a resize rewraps every line"
    );

    // and a run gaining an entry is a new history, at any width
    state.history_cache = Some((state.history_cache.take().unwrap().0, poison.clone()));
    let mut run = a_run();
    run.entries.push(agents::RunEntry {
        tool: None,
        revision: 5,
        event_id: None,
        kind: agents::RunEntryKind::Question,
        text: "Ship it?".to_owned(),
        created_at_ms: None,
    });
    state.fixture_pipe(PipeMessage::new(
        PipeSource::Plugin(yinz_tile::prelude::PluginId::new(0)),
        "agent_run_detail",
        &Some(serde_json::to_string(&run).unwrap()),
        &None,
        false,
    ));
    assert_ne!(state.cached_history_lines(40), &poison);
}

/// Better nothing than a column of single words: a half too narrow to wrap into says so
/// instead, and the divider is still drawn.
#[test]
fn a_half_too_narrow_to_wrap_into_draws_nothing() {
    let mut state = State::default();
    state.set_rows(with_summary(strip(), 3, "Ran the tests.", false));
    assert!(state.history_lines(12).is_empty());
    assert!(!state.history_lines(38).is_empty());
}

#[test]
fn live_and_archived_continuations_render_in_a_narrow_panel() {
    for archived in [false, true] {
        let mut run = a_run();
        run.origin_run_id = Some("claude:aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa".into());
        run.previous_run_id = Some("claude:bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb".into());
        let row = if archived {
            run_row(run.id.clone(), "history")
        } else {
            run.ended_at_ms = None;
            run.outcome = agents::RunOutcome::Active;
            StripRow::Agent {
                pane_id: 3,
                text: "↳ claude · history".into(),
                kind: "claude".into(),
                waiting: false,
            }
        };
        let mut state = State::default();
        state.set_rows(unfocused(vec![row]));
        state.set_run(run);
        state.render(59, 32);
        let header = &state.history().header;
        assert!(header.iter().all(|row| row.text.chars().count() <= 32));
        for label in ["started", "session", "origin", "previous"] {
            assert!(
                header.iter().any(|row| row.text.starts_with(label)),
                "{label} / archived={archived}"
            );
        }
        assert!(
            header.iter().any(|row| row.text.starts_with("        ")),
            "long session IDs wrap"
        );
        assert!(!state.history().entries.is_empty());
    }
}

/// An archived run with every cell filled, for the table tests.
fn cells(kind: &str, worktree: &str, title: &str, started_at_ms: Option<i64>) -> StripRow {
    StripRow::Run {
        run_id: format!("{kind}:{title}"),
        kind: kind.to_owned(),
        title: title.to_owned(),
        worktree: worktree.to_owned(),
        started_at_ms,
        continuation: false,
    }
}

/// What the table draws for one row, which is what a test can compare down a column.
fn drawn(columns: &Columns, now: i64, row: &StripRow) -> String {
    let StripRow::Run {
        kind,
        title,
        worktree,
        started_at_ms,
        continuation,
        ..
    } = row
    else {
        panic!("not a run row");
    };
    archive_row(
        columns,
        now,
        kind,
        worktree,
        title,
        *continuation,
        *started_at_ms,
    )
}

/// The point of the table: a column is read down, so a cell's start cannot depend on how
/// long the cell above it was.
#[test]
fn the_archive_table_lines_its_columns_up() {
    let now = 60_000;
    let rows = vec![
        cells("claude", "agent", "fix the pane exit teardown", Some(now)),
        cells(
            "codex",
            "yinzer",
            "add the archive table",
            Some(now - 7_200_000),
        ),
    ];
    let columns = archive_columns(&rows, 60);
    assert_eq!(
        (columns.age, columns.kind, columns.worktree),
        (true, 6, 6),
        "the widest cell in each column, and the age gutter",
    );
    let drawn: Vec<String> = rows.iter().map(|row| drawn(&columns, now, row)).collect();
    assert_eq!(
        drawn,
        [
            "now claude  agent   fix the pane exit teardown",
            " 2h codex   yinzer  add the archive table",
        ],
    );
    for row in &drawn {
        assert_eq!(
            row.find("fix").or_else(|| row.find("add")),
            Some(agents::AGE_WIDTH + 6 + COLUMN_GAP + 6 + COLUMN_GAP),
            "every prompt starts in the same column",
        );
    }
}

/// A pane too narrow for the lot sheds the agent first and the worktree next, because the
/// prompt is what the row is for and the kind is the most guessable thing on it.
#[test]
fn a_narrow_pane_sheds_the_agent_then_the_worktree_then_the_age() {
    let now = 0;
    let rows = vec![cells(
        "claude",
        "agent",
        "fix the pane exit teardown",
        Some(now),
    )];
    let widths = |cols: usize| {
        let columns = archive_columns(&rows, cols);
        (
            columns.age,
            columns.kind,
            columns.worktree,
            drawn(&columns, now, &rows[0]),
        )
    };
    assert_eq!(
        widths(48),
        (
            true,
            6,
            5,
            "now claude  agent  fix the pane exit teardow".to_owned()
        ),
        "one column short of the whole prompt, so the cut is the tail and nothing else",
    );
    assert_eq!(
        widths(30),
        (true, 0, 5, "now agent  fix the pane ex".to_owned()),
        "the agent goes first",
    );
    assert_eq!(
        widths(22),
        (true, 0, 0, "now fix the pane e".to_owned()),
        "then the worktree",
    );
    assert_eq!(
        widths(14),
        (false, 0, 0, "fix the pa".to_owned()),
        "then the age, and the prompt is cut to whatever is left",
    );
}

/// A row from before the archive recorded a start still has to hold its place in the
/// columns, or every cell to the right of it steps left on that one line.
#[test]
fn a_run_that_cannot_say_when_it_started_keeps_the_columns_in_line() {
    let rows = vec![
        cells("claude", "agent", "newer", Some(0)),
        cells("claude", "agent", "older", None),
    ];
    let columns = archive_columns(&rows, 60);
    let drawn: Vec<String> = rows.iter().map(|row| drawn(&columns, 0, row)).collect();
    assert_eq!(
        drawn,
        ["now claude  agent  newer", "    claude  agent  older"]
    );
}

/// The chain marker lives in the prompt cell now - the kind has a column of its own, so
/// there is nothing for it to elide.
#[test]
fn a_continuation_is_marked_inside_the_prompt_cell() {
    let mut row = cells(
        "claude",
        "agent",
        "continued after a context clear",
        Some(0),
    );
    let StripRow::Run { continuation, .. } = &mut row else {
        panic!("not a run row");
    };
    *continuation = true;
    let columns = archive_columns(&[row.clone()], 60);
    assert_eq!(
        drawn(&columns, 0, &row),
        "now claude  agent  ↳ continued after a context clear",
    );
}

/// A search hit is drawn in the archive's columns, so the hits can be read down them too -
/// with what matched first, since the cell is narrow and why a row matched is its point.
#[test]
fn a_search_hit_fills_the_same_cells() {
    let mut state = State {
        search_base: Some((Vec::new(), BTreeMap::new())),
        search_pending: true,
        search_request: 0,
        ..State::default()
    };
    assert!(state.apply_search_result(wire::SearchResult {
        request_id: "0".to_owned(),
        runs: vec![wire::SearchRun {
            run_id: "claude:session-1".to_owned(),
            kind: "claude".to_owned(),
            title: "hook history".to_owned(),
            revision: 4,
            snippet: "the matching line".to_owned(),
            started_at_ms: 10,
            worktree: "agent".to_owned(),
        }],
        truncated: false,
        error: String::new(),
    }));
    assert_eq!(
        state.rows,
        [StripRow::Run {
            run_id: "claude:session-1".to_owned(),
            kind: "claude".to_owned(),
            title: "the matching line — hook history".to_owned(),
            worktree: "agent".to_owned(),
            started_at_ms: Some(10),
            continuation: false,
        }],
    );
}

/// An archived run row as agent sends one: the cells a test does not name are the ones it
/// is not about.
fn run_row(run_id: impl Into<String>, title: &str) -> StripRow {
    StripRow::Run {
        run_id: run_id.into(),
        kind: "claude".to_owned(),
        title: title.to_owned(),
        worktree: String::new(),
        started_at_ms: None,
        continuation: false,
    }
}

/// One `AgentRun`, as agent pipes it over for the selected row.
fn a_run() -> agents::AgentRun {
    agents::AgentRun {
        id: "claude:session-1".to_owned(),
        pane_id: 3,
        yinz_session_name: "yinz".to_owned().into(),
        host_id: yinz_tile::prelude::HostId::default(),
        origin_run_id: None,
        previous_run_id: None,
        session_id: "session-1".to_owned(),
        kind: "claude".to_owned(),
        cwd: Some("/work".to_owned()),
        host: None,
        bubblewrap: None,
        started_at_ms: 10,
        ended_at_ms: Some(20),
        outcome: agents::RunOutcome::Completed,
        revision: 4,
        summarized_revision: 4,
        summary: Some(agents::Summary {
            text: "History is stored and rendered.".to_owned(),
            title: "hook history".to_owned(),
            awaiting: false,
            hash: 0,
            created_at_ms: None,
        }),
        entries: vec![
            agents::RunEntry {
                tool: None,
                revision: 1,
                event_id: None,
                kind: agents::RunEntryKind::Prompt,
                text: "implement history".to_owned(),
                created_at_ms: None,
            },
            agents::RunEntry {
                tool: None,
                revision: 2,
                event_id: None,
                kind: agents::RunEntryKind::Question,
                text: "Keep old runs?".to_owned(),
                created_at_ms: None,
            },
            agents::RunEntry {
                tool: None,
                revision: 3,
                event_id: None,
                kind: agents::RunEntryKind::Step,
                text: "Bash cargo test".to_owned(),
                created_at_ms: None,
            },
            agents::RunEntry {
                tool: None,
                revision: 4,
                event_id: None,
                kind: agents::RunEntryKind::Result,
                text: "Implemented and tested.".to_owned(),
                created_at_ms: None,
            },
        ],
        omitted_entries: 0,
    }
}
// -- a prompt a harness wrote --------------------------------------------

/// A notification of the shape Claude Code submits when a background task wakes an agent.
fn task_notification() -> String {
    [
        "<task-notification>",
        "<task-id>a10fe836c6a9fa4ca</task-id>",
        "<output-file>/private/tmp/claude-501/x/a10fe836c6a9fa4ca.output</output-file>",
        "<status>completed</status>",
        "<summary>Agent \"/code-review\" finished</summary>",
        "<note>A task-notification fires each time this agent stops.",
        "The same task-id may notify more than once.</note>",
        "<result>The review found four issues.",
        "Two are in the scanner.</result>",
        "</task-notification>",
    ]
    .join("\n")
}

/// [`task_notification`] with a `note` and a `result` long enough to scroll through: what
/// a real one carries, where the fold rows exist precisely because the body does not fit.
fn long_notification() -> String {
    // Paragraphs rather than lines: `markdown::render` reflows a run of single-newline
    // lines into one, and forty of them come back as fourteen rows.
    let long = |what: &str| {
        (0..40)
            .map(|line| format!("{what} line {line}."))
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    [
        "<task-notification>",
        "<task-id>a10fe836c6a9fa4ca</task-id>",
        "<output-file>/private/tmp/claude-501/x/a10fe836c6a9fa4ca.output</output-file>",
        "<status>completed</status>",
        "<summary>Agent \"/code-review\" finished</summary>",
        &format!("<note>{}</note>", long("Note")),
        &format!("<result>{}</result>", long("Result")),
        "</task-notification>",
    ]
    .join("\n")
}

/// A run holding one prompt, with the reader open on it.
fn reading(prompt: &str) -> State {
    reading_entry(agents::RunEntryKind::Prompt, prompt)
}

#[test]
fn reader_traverses_the_panel_bar_without_losing_its_event() {
    let mut state = reading("An event kept open while visiting Todos");
    state.on_key(KeyWithModifier::new(BareKey::Tab));
    assert!(state.tabs_focused);
    state.on_key(KeyWithModifier::new(BareKey::Right));
    assert!(state.tabs_focused);
    state.on_key(KeyWithModifier::new(BareKey::Left));
    state.on_key(KeyWithModifier::new(BareKey::Tab).with_shift_modifier());
    assert!(!state.tabs_focused);
    assert!(state.reading.is_some());
    assert!(rows(&state).join("\n").contains("An event kept open"));
}

/// [`reading`], for an event of some other kind.
fn reading_entry(kind: agents::RunEntryKind, text: &str) -> State {
    let mut state = State::default();
    state.set_rows(one_run_selected());
    let mut run = a_run();
    run.summary = None;
    // A `Result` reaches the timeline only on a run whose newest entry is past its last
    // summary - see `agents::run_history`. Every other kind is drawn either way.
    run.summarized_revision = 0;
    run.entries = vec![agents::RunEntry {
        tool: None,
        revision: 1,
        event_id: None,
        kind,
        text: text.to_owned(),
        created_at_ms: None,
    }];
    state.set_run(run);
    state.render(20, 50);
    state.focus = Focus::History;
    state.on_key(KeyWithModifier::new(BareKey::Enter));
    state.render(20, 50);
    state
}

fn rows(state: &State) -> Vec<String> {
    state
        .reader_lines()
        .iter()
        .map(|line| line.text.clone())
        .collect()
}

fn press(state: &mut State, key: BareKey) {
    state.on_key(KeyWithModifier::new(key));
    state.render(20, 50);
}

/// The four-row cap on the timeline is only honest because this exists. `Enter` opens the
/// event into its parts, with the short ones showing and the long ones shut — and `Enter`
/// again on one opens it.
#[test]
fn the_reader_folds_a_task_notification_and_enter_opens_a_section() {
    let mut state = reading(&task_notification());
    let folds = &state.reading.as_ref().expect("a reader").sections;
    assert_eq!(
        folds.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
        [
            "status",
            "task-id",
            "summary",
            "output-file",
            "note",
            "result"
        ]
    );

    let shut = rows(&state).join("\n");
    assert!(shut.contains("completed"), "{shut}");
    assert!(shut.contains("a10fe836c6a9fa4ca"), "{shut}");
    assert!(shut.contains("▸ result"), "{shut}");
    assert!(!shut.contains("The review found four issues."), "{shut}");

    // Walk to `result` and open it.
    for _ in 0..5 {
        press(&mut state, BareKey::Char('j'));
    }
    assert_eq!(state.reading.as_ref().expect("a reader").cursor, 5);
    press(&mut state, BareKey::Enter);
    let open = rows(&state).join("\n");
    assert!(open.contains("▾ result"), "{open}");
    assert!(open.contains("The review found four issues."), "{open}");
    // And the fold beside it stayed shut.
    assert!(!open.contains("notify more than once"), "{open}");
}

/// `j`/`k` walk a fold where there are folds and a line where there are not — the same
/// split the history half makes between an event and a line, and for the same reason.
#[test]
fn j_and_k_walk_the_folds_of_a_folding_reader_and_the_lines_of_every_other() {
    let mut state = reading(&task_notification());
    press(&mut state, BareKey::Char('j'));
    let folding = state.reading.as_ref().expect("a reader");
    assert_eq!(folding.cursor, 1);
    assert_eq!(folding.scroll, 0, "walking a fold is not scrolling");

    let mut prose = reading(&"word ".repeat(200));
    assert!(prose
        .reading
        .as_ref()
        .expect("a reader")
        .sections
        .is_empty());
    press(&mut prose, BareKey::Char('j'));
    assert_eq!(prose.reading.as_ref().expect("a reader").scroll, 1);
}

/// The folds are a reading of the event; `v` is how you get back to the event. A reader
/// that could not would be asking to be trusted.
#[test]
fn v_shows_the_event_exactly_as_it_arrived() {
    let mut state = reading(&task_notification());
    press(&mut state, BareKey::Char('v'));
    let raw = rows(&state).join("\n");
    assert!(raw.contains("<task-notification>"), "{raw}");
    assert!(
        raw.contains("<task-id>a10fe836c6a9fa4ca</task-id>"),
        "{raw}"
    );

    press(&mut state, BareKey::Char('v'));
    let folded = rows(&state).join("\n");
    assert!(!folded.contains("<task-notification>"), "{folded}");
    assert!(folded.contains("▸ result"), "{folded}");
}

/// Opening a fold is a different body, not a different view of one, so it has to miss the
/// cache the way a resize does.
#[test]
fn toggling_a_fold_rewraps_the_body() {
    let mut state = reading(&task_notification());
    let poison = envelope::ReaderLines {
        lines: vec![markdown::MdLine::plain("poison")],
        starts: vec![0],
    };
    let key = state.reader_cache.as_ref().expect("a cache").0;
    state.reader_cache = Some((key, poison.clone()));
    state.fill_reader_cache(50);
    assert_eq!(state.reader_lines(), poison.lines, "same event, same fold");

    // Onto `result`, which is the first fold with a body to hide. Walking there is not a
    // new body: the cursor is drawn from the spans, which the same rows already carry.
    for _ in 0..5 {
        state.on_key(KeyWithModifier::new(BareKey::Char('j')));
    }
    state.fill_reader_cache(50);
    assert_eq!(
        state.reader_lines(),
        poison.lines,
        "moving the cursor is not a new body"
    );

    state.on_key(KeyWithModifier::new(BareKey::Enter));
    state.fill_reader_cache(50);
    assert_ne!(state.reader_lines(), poison.lines, "a fold rewraps it");

    // And a one-line field has nothing to fold away, so Enter on one buys no rewrap.
    let key = state.reader_cache.as_ref().expect("a cache").0;
    state.reader_cache = Some((key, poison.clone()));
    for _ in 0..5 {
        state.on_key(KeyWithModifier::new(BareKey::Char('k')));
    }
    state.on_key(KeyWithModifier::new(BareKey::Enter));
    state.fill_reader_cache(50);
    assert_eq!(state.reader_lines(), poison.lines, "nothing to fold");
}

/// The whole reason the path is on the row at all. From the reader it is the fold under the
/// cursor; from the history half it is the event's, because a collapsed row has a cursor on
/// an event and none within one.
#[test]
fn o_on_a_task_notification_names_the_file_it_would_open() {
    let mut state = reading(&task_notification());
    for _ in 0..3 {
        press(&mut state, BareKey::Char('j'));
    }
    assert_eq!(
        state.open_target().as_deref(),
        Some("/private/tmp/claude-501/x/a10fe836c6a9fa4ca.output")
    );

    // A fold that names no file offers none.
    press(&mut state, BareKey::Char('j'));
    assert_eq!(state.open_target(), None);

    // And from the history half, with no reader open at all.
    state.on_key(KeyWithModifier::new(BareKey::Esc));
    assert!(state.reading.is_none());
    assert_eq!(
        state.open_target().as_deref(),
        Some("/private/tmp/claude-501/x/a10fe836c6a9fa4ca.output")
    );
}

/// The path names a file in another machine's temp folder. Opening it here would either
/// fail or, worse, open something else entirely.
#[test]
fn a_run_on_another_host_offers_no_file_to_open() {
    let mut state = reading(&task_notification());
    for _ in 0..3 {
        press(&mut state, BareKey::Char('j'));
    }
    assert!(state.open_target().is_some(), "the local run offers one");

    state.run.as_mut().expect("a run").host = Some("buildbox".to_owned());
    assert_eq!(state.open_target(), None);
    // `o` says where the file is rather than doing nothing, and leaves the panel up.
    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('o'))));
    assert_eq!(state.panel_hides, 0);
    assert_eq!(state.open_note.as_deref(), Some("file is on buildbox"));
    assert!(state.reader_label().starts_with("file is on buildbox"));
}

/// Where a file is on another machine, `o` says which one - but only where there is a
/// file: a fold or an event that names none names none on any machine.
#[test]
fn o_on_another_hosts_run_says_where_only_when_there_is_a_file() {
    let mut state = reading(&task_notification());
    state.run.as_mut().expect("a run").host = Some("buildbox".to_owned());
    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('o'))));
    assert_eq!(state.open_note.as_deref(), Some("no file named here"));

    let mut state = reading("fix the flaky reconnect test");
    state.run.as_mut().expect("a run").host = Some("buildbox".to_owned());
    state.on_key(KeyWithModifier::new(BareKey::Esc));
    assert_eq!(state.focus, Focus::History);
    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('o'))));
    assert_eq!(state.open_note.as_deref(), Some("no file named here"));
    assert_eq!(state.panel_hides, 0);
}

/// `y` copies the fold under the `>` in a folding reader, and the whole event exactly as it
/// arrived from the source `v` puts up or from a prose event - so `v` then `y` is how a
/// folded event is copied whole.
#[test]
fn y_copies_the_fold_under_the_cursor_and_the_whole_event_otherwise() {
    let mut state = reading(&task_notification());
    assert_eq!(state.copy_target().as_deref(), Some("completed"));

    for _ in 0..5 {
        press(&mut state, BareKey::Char('j'));
    }
    assert_eq!(
        state.copy_target().as_deref(),
        Some("The review found four issues.\nTwo are in the scanner.")
    );

    // Showing the source is asking for the source.
    press(&mut state, BareKey::Char('v'));
    assert_eq!(
        state.copy_target().as_deref(),
        Some(task_notification().as_str())
    );

    let prose = reading("fix the flaky reconnect test");
    assert_eq!(
        prose.copy_target().as_deref(),
        Some("fix the flaky reconnect test")
    );
}

#[test]
fn y_copies_the_selected_history_event_without_opening_it() {
    for text in [
        "**Agent reply**\n\n你好 🌍\n".repeat(80),
        task_notification(),
    ] {
        let mut state = reading(&text);
        press(&mut state, BareKey::Esc);
        assert!(state.reading.is_none());
        assert_eq!(state.copy_target().as_deref(), Some(text.as_str()));
        assert!(state.footer_hint().unwrap().contains("y copy"));
        assert!(state
            .key_sheet()
            .iter()
            .any(|(key, description)| key == "y" && description == "copy the whole event"));
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('y'))));
        assert_eq!(state.open_note.as_deref(), Some("copied the whole event"));
        assert!(state.reading.is_none());

        state.on_key(KeyWithModifier::new(BareKey::Tab).with_shift_modifier());
        assert_eq!(state.focus, Focus::List);
        assert!(!state.on_key(KeyWithModifier::new(BareKey::Char('y'))));
        assert!(state.open_note.is_none());
    }

    let mut empty = State {
        focus: Focus::History,
        ..Default::default()
    };
    assert!(!empty.on_key(KeyWithModifier::new(BareKey::Char('y'))));
    assert!(empty.open_note.is_none());
}

/// A copy is invisible until it is pasted, and the fold and the whole event are two
/// different things to paste: `y` says which it took, and how to get the other, on the
/// divider until the next key.
#[test]
fn y_says_what_it_copied() {
    let mut state = reading(&task_notification());
    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('y'))));
    assert!(
        state
            .reader_label()
            .starts_with("copied status · v then y copies the whole event · prompt"),
        "{}",
        state.reader_label()
    );
    press(&mut state, BareKey::Char('v'));
    assert!(state.open_note.is_none(), "the next key takes it down");
    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('y'))));
    assert!(
        state
            .reader_label()
            .starts_with("copied the whole event · prompt"),
        "{}",
        state.reader_label()
    );
}

/// A line scroll is how a long fold is read, and the cursor comes along with it: what
/// `Enter`, `y` and `o` act on is always something on screen. Left at the head of the
/// event, the cursor pointed at a fold the user had walked ten pages away from, with no
/// `>` anywhere to say which one - and `k` on it did nothing at all, having nowhere left
/// to go.
#[test]
fn scrolling_the_reader_takes_the_cursor_with_it() {
    let mut state = reading(&long_notification());
    // Onto `result` and open it, so there is more body than the pane has room for.
    for _ in 0..5 {
        press(&mut state, BareKey::Char('j'));
    }
    press(&mut state, BareKey::Enter);
    // Back to the head of the event, which the cursor comes back to with it.
    for _ in 0..80 {
        press(&mut state, BareKey::Up);
    }
    let reader = state.reading.as_ref().expect("a reader");
    assert_eq!((reader.scroll, reader.cursor), (0, 0));

    for _ in 0..8 {
        press(&mut state, BareKey::Down);
    }
    let starts = state.reader_starts().to_vec();
    let reader = state.reading.as_ref().expect("a reader");
    assert_eq!(reader.scroll, 8, "eight lines down into the event");
    assert_ne!(
        reader.cursor, 0,
        "and off the fold the reader opened on, which is what `Enter` would have acted on"
    );
    assert_eq!(
        Some(reader.cursor),
        state.fold_at(reader.scroll),
        "the cursor is the fold owning the top row"
    );
    assert!(starts[reader.cursor] <= reader.scroll);
}

/// A fold taller than the pane is shown from its beginning - the row carrying its name.
/// Without the winding it opened where it sat, four rows from the foot, and spent the rest
/// of itself below the screen.
#[test]
fn opening_a_fold_shows_it_from_its_first_row() {
    let mut state = reading(&long_notification());
    for _ in 0..5 {
        press(&mut state, BareKey::Char('j'));
    }
    press(&mut state, BareKey::Enter);

    let starts = state.reader_starts().to_vec();
    let reader = state.reading.as_ref().expect("a reader");
    assert_eq!(reader.cursor, 5);
    assert_eq!(reader.scroll, starts[5], "the fold's name is the top row");
}

/// The reader's parse depends on the event's text alone, which an opening never changes,
/// so leaving the source with `v` and coming back re-wraps the parse made the first time
/// rather than parsing the whole event again. Read by poisoning that parse.
#[test]
fn toggling_raw_keeps_the_readers_parse() {
    let mut state = reading(&task_notification());
    press(&mut state, BareKey::Char('v'));
    let id = state.reading.as_ref().expect("a reader").id;
    assert_eq!(
        state.reader_blocks.as_ref().map(|(held, _)| *held),
        Some(id)
    );

    state.reader_blocks = Some((id, markdown::parse("poison").into()));
    press(&mut state, BareKey::Char('v'));
    press(&mut state, BareKey::Char('v'));
    let lines: Vec<&str> = state
        .reader_lines()
        .iter()
        .map(|line| line.text.as_str())
        .collect();
    assert_eq!(lines, ["poison"]);
}

/// A fold's parse depends on the section and whether it is open, and nothing else: a
/// resize re-wraps what is held, a fold toggled parses that fold alone. Read by poisoning
/// one fold's parse, as `toggling_raw_keeps_the_readers_parse` does the prose one.
#[test]
fn each_fold_is_parsed_once_per_toggle_and_rewrapped_per_width() {
    let mut state = reading(&task_notification());
    let folds = &mut state.reader_folds.as_mut().expect("held folds").1;
    assert_eq!(folds.len(), 6);
    folds[4].1 = markdown::parse("poison");
    state.render(20, 44);
    let has_poison = |state: &State| rows(state).iter().any(|row| row.contains("poison"));
    assert!(has_poison(&state), "a resize re-wraps what is held");

    for _ in 0..5 {
        press(&mut state, BareKey::Char('j'));
    }
    press(&mut state, BareKey::Enter);
    assert!(
        rows(&state)
            .join("\n")
            .contains("The review found four issues."),
        "the fold toggled is parsed open"
    );
    assert!(has_poison(&state), "and the one beside it is left alone");

    press(&mut state, BareKey::Char('k'));
    press(&mut state, BareKey::Enter);
    assert!(!has_poison(&state), "until it is toggled itself");
    assert!(rows(&state).join("\n").contains("▾ note"));
}

/// An event the timeline parsed whole is opened on that parse rather than a new one, and
/// the reader holds the event's own text rather than a copy. One it parsed only in part,
/// or drew from a digest, is parsed when the reader first draws it.
#[test]
fn a_short_event_opens_on_the_timelines_parse() {
    let state = reading_entry(agents::RunEntryKind::Message, "Shall I **push** it?");
    let opened = state.reading.as_ref().expect("a reader");
    let parsed = &state.run.as_ref().expect("the run").parsed;
    let shared = parsed.whole_blocks(&opened.text).expect("parsed whole");
    let (_, held) = state.reader_blocks.as_ref().expect("the reader's parse");
    assert!(Arc::ptr_eq(held, &shared));
    assert!(Arc::ptr_eq(
        &opened.text,
        &state.cursor_event().expect("an event").text
    ));
    assert!(rows(&state).join("\n").contains("Shall I push it?"));

    for state in [
        reading_entry(agents::RunEntryKind::Message, &"word ".repeat(4_000)),
        reading(&task_notification()),
    ] {
        let opened = state.reading.as_ref().expect("a reader");
        let parsed = &state.run.as_ref().expect("the run").parsed;
        assert!(parsed.whole_blocks(&opened.text).is_none());
    }
}

/// `v` puts up a different document, so there is no row to hold the place at and the
/// reader starts at the head of it. The cursor goes back with the scroll: left where it
/// was, it pointed forty rows below a window anchored to zero, where nothing draws it and
/// `Enter`, `y` and `o` would all have acted on a fold nobody could see.
#[test]
fn coming_back_from_raw_starts_at_the_top() {
    let mut state = reading(&long_notification());
    // Open `note`, which is long enough to push `result` off the foot, and put the cursor
    // on `result` under it.
    for _ in 0..4 {
        press(&mut state, BareKey::Char('j'));
    }
    press(&mut state, BareKey::Enter);
    press(&mut state, BareKey::Char('j'));
    let starts = state.reader_starts().to_vec();
    assert_eq!(state.reading.as_ref().expect("a reader").cursor, 5);
    assert!(
        starts[5] > state.last_reader_rows,
        "the fold is off the foot at a scroll of zero"
    );

    press(&mut state, BareKey::Char('v'));
    press(&mut state, BareKey::Char('v'));
    let reader = state.reading.as_ref().expect("a reader");
    assert_eq!((reader.scroll, reader.cursor), (0, 0));
}

/// A prompt is the one kind a harness writes as well as a person - the same gate the
/// timeline puts on its digest. An agent writing *about* an envelope is writing prose, and
/// folding it away would hide the message the reader was opened to read.
#[test]
fn a_result_that_quotes_an_envelope_opens_as_prose() {
    let text = format!(
        "I removed the handler. It was woken by this:\n\n{}",
        task_notification()
    );
    let state = reading_entry(agents::RunEntryKind::Result, &text);

    let reader = state.reading.as_ref().expect("a reader");
    assert!(
        reader.sections.is_empty(),
        "no folds on an agent's own words"
    );
    assert_eq!(reader.envelope, None, "and nothing to name on the divider");
    assert_eq!(
        rows(&state),
        markdown::render(&text, 48, markdown::Options::compact())
            .iter()
            .map(|line| line.text.clone())
            .collect::<Vec<_>>()
    );

    // And the prompt beside it is still read apart, which is the half of the gate that
    // must not have moved.
    let prompt = reading(&task_notification());
    assert!(!prompt
        .reading
        .as_ref()
        .expect("a reader")
        .sections
        .is_empty());
}

/// The regression guard: everything a person typed reads, scrolls and copies exactly as it
/// did before any of this existed.
#[test]
fn a_prompt_with_no_envelope_reads_exactly_as_it_did_before() {
    let prose = "fix the flaky reconnect test\n\n- check the timeout\n- check the retry";
    let mut state = reading(prose);
    let reader = state.reading.as_ref().expect("a reader");
    assert!(reader.sections.is_empty());
    assert!(!reader.raw);
    assert_eq!(state.reader_starts(), &[] as &[usize]);
    assert_eq!(
        rows(&state),
        markdown::render(prose, 48, markdown::Options::compact())
            .iter()
            .map(|line| line.text.clone())
            .collect::<Vec<_>>()
    );

    // `v` and `Enter` have nothing to act on and say so by doing nothing.
    let before = rows(&state);
    press(&mut state, BareKey::Char('v'));
    press(&mut state, BareKey::Enter);
    assert_eq!(rows(&state), before);
}

// -- the keys a view names ------------------------------------------------

/// A question's answer keys are its tail, and a narrow divider cuts the tail: the footer
/// asks answer first at every width the panel's half comes in, the divider puts the
/// question before the run's name, and a pane with no history half still asks.
#[test]
fn the_resume_question_keeps_its_answer_keys_at_every_width() {
    let mut state = State::default();
    state.set_rows(unfocused(archive()));
    state.selected = 1;
    state.set_run(a_run());
    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('r'))));
    for cols in 22..=40 {
        state.render(11, cols);
        let footer = state.footer_hint().unwrap();
        assert!(footer.starts_with("y resume · n"), "{footer:?} at {cols}");
    }
    state.render(7, 30);
    assert_eq!(state.last_history_rows, 0, "no divider to ask on");
    assert_eq!(state.footer_hint().as_deref(), Some("y resume · n cancel"));
    state.render(11, 24);
    let line = divider_line(&state.history_label(), 5, 0, 24);
    assert!(line.contains("y/n"), "{line}");
}

/// A refusal is wrapped whole under the divider, which a narrow half cuts to a few words
/// of it, and the timeline keeps a row under it.
#[test]
fn a_refusal_is_wrapped_whole_under_the_divider() {
    let mut state = State::default();
    state.set_rows(unfocused(archive()));
    state.selected = 1;
    state.set_run(a_run());
    let detail = "that run is live again in pane 12, and resuming it would put two agents on \
                  one conversation";
    state.resume_note = Some(("run:claude:session-1".into(), detail.into()));
    let lines = state.refusal_lines(24);
    assert!(lines.len() > 2, "{lines:?}");
    assert!(lines.iter().all(|line| line.width() <= 22), "{lines:?}");
    assert_eq!(lines.join(" "), format!("resume failed: {detail}"));

    state.render(20, 24);
    let header = state.history().header.len() + 1;
    let held = lines.len().min(state.last_history_rows - header - 1);
    assert_eq!(state.last_history_body_top, header + held);
    assert_eq!(
        state.last_history_body_rows,
        state.last_history_rows - header - held
    );
    assert!(state
        .footer_hint()
        .unwrap()
        .starts_with("resume failed: that run"));
    assert_eq!(
        wrap_words("a veryveryverylongword b", 5),
        ["a", "veryv", "eryve", "rylon", "gword", "b"]
    );
}

/// A resume that worked has walked the keyboard into the new pane, under the panel: the
/// panel goes, wherever the cursor has got to since the question was answered.
#[test]
fn a_successful_resume_puts_the_panel_away() {
    let succeeded = || {
        let result = wire::ResumeResult {
            run_id: "claude:session-1".to_owned(),
            detail: String::new(),
        };
        PipeMessage::new(
            PipeSource::Plugin(yinz_tile::prelude::PluginId::new(0)),
            "agent_resume_result",
            &Some(serde_json::to_string(&result).expect("encoded")),
            &None,
            false,
        )
    };
    for walked_on in [false, true] {
        let mut state = State::default();
        state.set_rows(unfocused(archive()));
        state.selected = 1;
        state.on_key(KeyWithModifier::new(BareKey::Char('r')));
        state.on_key(KeyWithModifier::new(BareKey::Char('y')));
        assert_eq!(state.resume_requests, ["claude:session-1"]);
        if walked_on {
            state.on_key(KeyWithModifier::new(BareKey::Down));
        }
        assert!(state.fixture_pipe(succeeded()));
        assert_eq!(state.panel_hides, 1, "walked on: {walked_on}");
    }
}

/// `o` opens the file for this client - floating, focused and shown - and puts the panel
/// away first, since the editor is under it.
#[test]
fn o_opens_the_named_file_and_puts_the_panel_away() {
    let mut state = reading(&task_notification());
    for _ in 0..3 {
        press(&mut state, BareKey::Char('j'));
    }
    assert!(!state.on_key(KeyWithModifier::new(BareKey::Char('o'))));
    assert_eq!(state.panel_hides, 1);
    assert_eq!(
        state.opened_files,
        ["/private/tmp/claude-501/x/a10fe836c6a9fa4ca.output"]
    );
    assert!(
        state.reading.is_some(),
        "the event is still open when the panel comes back"
    );
}

/// Where there is no file, `o` says so - on the reader's divider or the history half's -
/// and the next key takes the note down.
#[test]
fn o_on_a_fold_with_no_file_says_so() {
    let mut state = reading(&task_notification());
    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('o'))));
    assert_eq!(state.panel_hides, 0);
    assert!(state.opened_files.is_empty());
    assert!(state
        .reader_label()
        .starts_with("no file named here · prompt"));
    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('j'))));
    assert!(state.open_note.is_none());

    let mut state = reading("fix the flaky reconnect test");
    state.on_key(KeyWithModifier::new(BareKey::Esc));
    assert_eq!(state.focus, Focus::History);
    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('o'))));
    state.fill_history_cache(50);
    assert!(state.history_label().starts_with("no file named here · "));
    assert!(
        state.on_key(KeyWithModifier::new(BareKey::Char('x'))),
        "a key that does nothing else still takes the note down"
    );
    assert!(state.open_note.is_none());
}

/// The reader's source toggle is `v`, so `r` in there neither shows the source nor arms a
/// resume - one `Esc` from a resume `y` answers.
#[test]
fn reader_r_does_not_show_the_source_or_arm_a_resume() {
    let mut state = reading(&task_notification());
    assert!(!state.on_key(KeyWithModifier::new(BareKey::Char('r'))));
    assert!(!state.reading.as_ref().expect("a reader").raw);
    assert!(state.confirming_resume.is_none());
    press(&mut state, BareKey::Char('v'));
    assert!(state.reader_label().starts_with("prompt · source"));
}

/// The search box scrolls sideways rather than losing its end, and the terminal's caret is
/// in it - where the next character typed goes.
#[test]
fn the_search_box_keeps_its_end_and_its_caret_in_view() {
    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    state.begin_search();
    state.search_query = "a query long enough to run off the end of the box".into();
    state.render(11, 24);
    let label = state.view_label().unwrap();
    assert!(label.starts_with("Search: "), "{label}");
    assert!(label.width() < 24, "{label}");
    assert!(label.ends_with("box"), "{label}");
    let (_, column) = state.search_box(24);
    assert!(column < 24);
    assert_eq!(state.text_cursor(10, 24), Some((column, 0)));
    state.has_keyboard = Some(false);
    assert_eq!(state.text_cursor(10, 24), None);
}

/// An empty list says whether anybody has told it so: before agent's first strip it is
/// waiting, and after one it says what to do about having no agents.
#[test]
fn an_empty_list_says_whether_agent_has_said_anything_yet() {
    let mut state = State::default();
    assert_eq!(state.empty_list_message(), ("waiting for agent…", None));
    state.render(11, 60);
    let footer = state.footer_hint().unwrap();
    assert!(
        !footer.contains("Tab bar"),
        "nobody said there is one: {footer}"
    );
    let empty = serde_json::to_string(&unfocused(Vec::new())).expect("encoded");
    assert!(
        state.fixture_pipe(PipeMessage::new(
            PipeSource::Plugin(yinz_tile::prelude::PluginId::new(0)),
            "agent_strip",
            &Some(empty),
            &None,
            false,
        )),
        "the first strip is news, empty or not"
    );
    assert_eq!(
        state.empty_list_message(),
        ("no agents", Some("Ctrl e n starts one"))
    );
    state.render(11, 60);
    let footer = state.footer_hint().unwrap();
    assert!(footer.contains("Tab bar"), "{footer}");
}

fn questions(count: u64) -> agents::AgentRun {
    let mut run = a_run();
    run.summary = None;
    run.entries = (0..count)
        .map(|index| agents::RunEntry {
            tool: None,
            revision: index + 1,
            event_id: None,
            kind: agents::RunEntryKind::Question,
            text: format!("question {index}"),
            created_at_ms: None,
        })
        .collect();
    run
}

/// The page keys and the wheel move the focused half: the window stays where they put it,
/// and the cursor comes to the window, so `Enter` and `o` act on something on screen.
#[test]
fn the_page_keys_scroll_the_focused_history_half() {
    let mut state = State::default();
    state.set_rows(one_run_selected());
    state.set_run(questions(40));
    state.focus = Focus::History;
    state.render(20, 50);
    let on_screen = |state: &State| {
        let end = state.history().entries.len() - state.history_scroll;
        let start = end - state.last_history_body_rows;
        let event = state.cursor_event().expect("a cursor");
        event.start >= start && event.end() <= end
    };
    assert!(state.on_key(KeyWithModifier::new(BareKey::PageUp)));
    state.render(20, 50);
    assert_eq!(state.history_scroll, state.page());
    assert!(on_screen(&state), "the cursor came with the window");
    assert!(state.on_key(KeyWithModifier::new(BareKey::PageUp)));
    state.render(20, 50);
    assert_eq!(state.history_scroll, 2 * state.page());
    assert!(on_screen(&state));
    for _ in 0..2 {
        assert!(state.on_key(KeyWithModifier::new(BareKey::PageDown)));
        state.render(20, 50);
    }
    assert_eq!(state.history_scroll, 0);
    assert!(on_screen(&state));
}

#[test]
fn the_wheel_scrolls_the_focused_history_half() {
    let mut state = State::default();
    state.set_rows(one_run_selected());
    state.set_run(questions(40));
    state.render(20, 50);
    let below = (state.last_list_rows + 3) as isize;
    assert!(state.on_mouse(Mouse::LeftClick((below) - 1, 0)));
    assert_eq!(state.focus, Focus::History);
    state.on_mouse(Mouse::Hover((below) - 1, 0));
    assert!(state.on_mouse(Mouse::ScrollUp(3)));
    state.render(20, 50);
    assert_eq!(
        state.history_scroll, 3,
        "kept, not wound back to the cursor"
    );
}

/// Taking the half's keys after wheeling it keeps the window where the wheel left it, and
/// puts the cursor on screen in it.
#[test]
fn taking_the_history_half_keeps_where_the_wheel_left_it() {
    let mut state = State::default();
    state.set_rows(one_run_selected());
    state.set_run(questions(40));
    state.render(20, 50);
    state.on_mouse(Mouse::Hover(((state.last_list_rows + 3) as isize) - 1, 0));
    assert!(state.on_mouse(Mouse::ScrollUp(5)));
    state.render(20, 50);
    assert_eq!(state.history_scroll, 5);
    assert!(state.on_key(KeyWithModifier::new(BareKey::Tab)));
    state.render(20, 50);
    assert_eq!(state.history_scroll, 5);
    let end = state.history().entries.len() - 5;
    let event = state.cursor_event().expect("a cursor");
    assert!(event.start >= end - state.last_history_body_rows && event.end() <= end);
}

/// The revision of the event the bottom row of the history's window belongs to.
fn bottom_revision(state: &State) -> Option<u64> {
    let end = state.history().entries.len() - state.history_scroll;
    let from_newest = state.event_at_row(end - 1)?;
    let events = &state.history().events;
    events[events.len() - from_newest - 1].revision
}

/// Off the tail the cursor holds the event it was put on, as a working agent adds more -
/// and as the oldest are trimmed off - while on the newest it follows the tail.
#[test]
fn the_cursor_holds_its_event_as_the_run_grows() {
    let mut state = State::default();
    state.set_rows(one_run_selected());
    state.set_run(questions(30));
    state.focus = Focus::History;
    state.render(20, 50);
    for _ in 0..2 {
        state.on_key(KeyWithModifier::new(BareKey::Char('k')));
    }
    state.render(20, 50);
    assert_eq!(state.cursor_event().unwrap().revision, Some(28));
    state.set_run(questions(31));
    state.render(20, 50);
    assert_eq!(state.cursor_event().unwrap().revision, Some(28));
    // The oldest trimmed off and a newer one on: the same event still.
    let mut trimmed = questions(32);
    trimmed.entries.remove(0);
    state.set_run(trimmed);
    state.render(20, 50);
    assert_eq!(state.cursor_event().unwrap().revision, Some(28));
    assert!(state.on_key(KeyWithModifier::new(BareKey::Enter)));
    assert_eq!(
        &*state.reading.as_ref().expect("a reader").text,
        "question 27"
    );

    // On the newest, the cursor follows the tail.
    let mut state = State::default();
    state.set_rows(one_run_selected());
    state.set_run(questions(30));
    state.focus = Focus::History;
    state.render(20, 50);
    state.set_run(questions(31));
    state.render(20, 50);
    assert_eq!(state.cursor_event().unwrap().revision, Some(31));
}

/// A window wound back from the tail holds the text being read as the run grows under it,
/// and as the pane is resized; at the tail it follows new events.
#[test]
fn a_wound_back_window_holds_still_as_the_run_grows() {
    let mut state = State::default();
    state.set_rows(one_run_selected());
    state.set_run(questions(30));
    state.render(20, 50);
    assert!(state.on_key(KeyWithModifier::new(BareKey::PageUp)));
    state.render(20, 50);
    let before = bottom_revision(&state);
    assert!(before.is_some());
    state.set_run(questions(33));
    state.render(20, 50);
    assert_eq!(bottom_revision(&state), before);
    state.render(20, 41);
    assert_eq!(bottom_revision(&state), before, "a resize moves nothing");

    while state.on_key(KeyWithModifier::new(BareKey::PageDown)) {
        state.render(20, 50);
    }
    state.render(20, 50);
    assert_eq!(state.history_scroll, 0);
    state.set_run(questions(35));
    state.render(20, 50);
    assert_eq!(state.history_scroll, 0, "the tail follows");
    assert_eq!(bottom_revision(&state), Some(35));
}

/// A search sent and not answered says it is searching, is not sent again by a second
/// Enter, and gives up after a while with the query still there to retry.
#[test]
fn a_search_nobody_answers_times_out_and_keeps_the_query() {
    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    state.begin_search();
    state.search_query = "flaky".into();
    assert!(state.search_key(KeyWithModifier::new(BareKey::Enter)));
    let request = state.search_request;
    assert!(state.search_key(KeyWithModifier::new(BareKey::Enter)));
    assert_eq!(state.search_request, request, "nothing sent twice");
    let late = wire::SearchResult {
        request_id: request.to_string(),
        ..Default::default()
    };
    assert!(!state.expire_search(state.search_deadline_ms - 1));
    assert!(state.expire_search(state.search_deadline_ms));
    assert!(!state.search_pending);
    assert!(state.search_editing);
    assert_eq!(state.search_query, "flaky");
    assert_eq!(state.search_error.as_deref(), Some("agent did not answer"));
    assert_eq!(
        state.search_failure_lines(24).join(" "),
        "failed: agent did not answer",
        "said under the box, whole"
    );
    assert!(!state.apply_search_result(late), "a late reply is ignored");
    assert!(state.search_key(KeyWithModifier::new(BareKey::Enter)));
    assert!(state.search_pending, "Enter asks again");
    assert!(state.search_error.is_none());
}

/// A failed search stays open on the query as typed, over the rows the last search found,
/// and `Esc` goes back to that search. `/` over a search edits its query, and `Ctrl-u`
/// clears it.
#[test]
fn a_failed_search_stays_open_on_its_query_and_esc_goes_back_to_the_last_one() {
    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    let reply = search_reply(&mut state, &["match", "another match"]);
    assert!(state.apply_search_result(reply));
    let matches = state.rows.clone();
    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('/'))));
    assert_eq!(
        state.search_query, "history",
        "/ edits the search it is over"
    );
    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('u')).with_ctrl_modifier()));
    assert!(state.search_query.is_empty());
    for ch in "bar".chars() {
        state.on_key(KeyWithModifier::new(BareKey::Char(ch)));
    }
    assert!(state.on_key(KeyWithModifier::new(BareKey::Enter)));
    let failed = wire::SearchResult {
        request_id: state.search_request.to_string(),
        error: "store busy".into(),
        ..Default::default()
    };
    assert!(state.apply_search_result(failed));
    assert!(state.search_editing);
    assert_eq!(state.search_query, "bar");
    assert_eq!(state.rows, matches, "the last search's rows stay");
    // The reason is under the box, whole, and the footer is the keys that go on from it.
    for cols in [24, 28, 34, 40] {
        state.render(11, cols);
        assert_eq!(
            state.footer_hint().as_deref(),
            Some("Enter retry · Esc back")
        );
        let lines = state.search_failure_lines(cols);
        assert_eq!(lines.join(" "), "failed: store busy", "{cols}");
        assert_eq!(state.last_label_rows, 1 + lines.len(), "{cols}");
    }
    // Clicks land on the list under the reason, not on the reason.
    state.render(11, 24);
    state.selected = state.rows.len() - 1;
    let first = state.rows.iter().position(selectable).unwrap();
    assert!(state.on_mouse(Mouse::LeftClick(
        (state.last_label_rows + first) as isize,
        0
    )));
    assert_eq!(state.selected, first);
    // A reason agent gives while it is starting up, long enough to need every row it gets.
    state.search_error =
        Some("agent has not registered this agent list yet - try again".to_owned());
    state.render(11, 24);
    let lines = state.search_failure_lines(24);
    assert!(lines.iter().all(|line| line.width() <= 24), "{lines:?}");
    assert_eq!(
        lines.join(" "),
        "failed: agent has not registered this agent list yet - try again"
    );
    assert_eq!(state.last_label_rows, 1 + lines.len());
    state.search_error = Some("store busy".to_owned());

    assert!(state.on_key(KeyWithModifier::new(BareKey::Esc)));
    assert_eq!(state.search_query, "history");
    assert!(state.search_error.is_none());
    assert!(!state.search_editing);
    state.render(11, 24);
    assert!(state.search_failure_lines(24).is_empty());
    assert_eq!(state.last_label_rows, 1, "nothing is left under the box");
}

/// The search box takes every key ahead of the tab bar, so it cannot stay open behind a
/// `Tab`: the key closes it, keeps what was typed for the next `/`, and goes on round the
/// ring from the list it sits over - into the history half, or with Shift to the tab bar.
#[test]
fn tab_in_the_search_box_keeps_the_draft_and_walks_the_ring() {
    let key = |ch: char| KeyWithModifier::new(BareKey::Char(ch));
    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    state.render(11, 60);
    assert!(state.on_key(key('/')));
    for ch in "ab".chars() {
        assert!(state.on_key(key(ch)));
    }
    assert!(state.on_key(KeyWithModifier::new(BareKey::Tab)));
    assert!(!state.search_editing);
    assert!(
        state.search_base.is_none(),
        "nothing was applied to go back to"
    );
    assert!(
        state.ring_yields == [wire::STOP_TABS] || state.focus == Focus::History,
        "on round the ring from the list"
    );
    assert_eq!(state.search_draft.as_deref(), Some("ab"));
    // Neither `w` nor `p` puts a search away when there is none: they leave views, and the
    // draft waits for the next `/` through both.
    assert!(state.on_key(key('w')));
    assert!(state.waiting_base.is_some());
    assert!(state.on_key(key('w')));
    assert!(state.waiting_base.is_none());
    assert_eq!(state.search_draft.as_deref(), Some("ab"), "w, w keeps it");
    assert!(matches!(
        state.rows.get(state.selected),
        Some(StripRow::Agent { .. })
    ));
    state.on_key(key('p'));
    assert!(state.predecessors.is_some());
    assert!(state.on_key(KeyWithModifier::new(BareKey::Esc)));
    assert!(state.predecessors.is_none());
    assert_eq!(state.search_draft.as_deref(), Some("ab"), "p, Esc keeps it");
    assert!(state.on_key(key('/')));
    assert_eq!(state.search_query, "ab", "the draft comes back");

    assert!(state.on_key(KeyWithModifier::new(BareKey::Tab).with_shift_modifier()));
    assert!(!state.search_editing);
    assert!(state.tabs_focused, "Shift+Tab from the box is the tab bar");
    state.tabs_focused = false;
    state.bar_focused = vec![];

    // Esc cancels as it always did, and keeps nothing.
    assert!(state.on_key(key('/')));
    assert_eq!(state.search_query, "ab");
    assert!(state.on_key(KeyWithModifier::new(BareKey::Esc)));
    assert!(!state.search_editing);
    assert!(state.on_key(key('/')));
    assert!(state.search_query.is_empty(), "Esc keeps no draft");
    assert!(state.on_key(KeyWithModifier::new(BareKey::Esc)));

    // Over a search already applied, Tab goes back to that search, and `c` puts the draft
    // away with it.
    let reply = search_reply(&mut state, &["match"]);
    assert!(state.apply_search_result(reply));
    assert!(state.on_key(key('/')));
    assert!(state.on_key(key('x')));
    assert!(state.on_key(KeyWithModifier::new(BareKey::Tab)));
    assert!(!state.search_editing);
    assert_eq!(
        state.search_query, "history",
        "the applied search, under the list"
    );
    assert_eq!(state.search_draft.as_deref(), Some("historyx"));
    assert!(state.on_key(key('c')));
    assert!(state.search_draft.is_none());
    assert!(state.on_key(key('/')));
    assert!(state.search_query.is_empty());
}

/// The hits are not run again, so once they are old enough to matter the view says how old.
#[test]
fn an_applied_search_says_how_old_it_is() {
    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    let reply = search_reply(&mut state, &["match"]);
    assert!(state.apply_search_result(reply));
    assert_eq!(state.view_label().as_deref(), Some("Search: history"));
    state.searched_at_ms = Some(now_ms() - 42 * 60_000);
    assert_eq!(
        state.view_label().as_deref(),
        Some("Search: history · 42m ago")
    );
    assert!(state.shows_an_age());
    assert!(state.clear_search());
    assert!(state.searched_at_ms.is_none());
}

/// Enter on the last page of `load older runs…` puts the cursor on the first run it loaded,
/// not back at the top of the list; a page that is not the last keeps it on the row.
#[test]
fn the_last_archive_page_leaves_the_cursor_on_the_first_run_it_loaded() {
    let archive_of = |runs: &[&str], more: bool| {
        let mut rows = strip();
        rows.push(StripRow::Project("archive".to_owned()));
        rows.extend(runs.iter().map(|id| run_row(*id, "a run")));
        if more {
            rows.push(StripRow::MoreRuns);
        }
        unfocused(rows)
    };
    let mut state = State::default();
    state.set_rows(archive_of(&["claude:1"], true));
    state.selected = state.rows.len() - 1;
    assert_eq!(state.history_placeholder(40), "Enter loads older runs");
    state.set_rows(archive_of(&["claude:1", "claude:2"], true));
    assert_eq!(state.selection_key().as_deref(), Some("more"));
    state.set_rows(archive_of(&["claude:1", "claude:2", "claude:3"], false));
    assert_eq!(state.selection_key().as_deref(), Some("run:claude:3"));
    assert_eq!(state.list_scroll, None);
}

/// The history half says which kind of empty it is.
#[test]
fn an_empty_history_says_which_kind_of_empty_it_is() {
    let mut state = State::default();
    state.set_rows(unfocused(archive()));
    state.selected = 1;
    assert_eq!(state.history_placeholder(40), "loading…");
    let mut empty = a_run();
    empty.summary = None;
    empty.entries.clear();
    state.set_run(empty);
    assert_eq!(state.history_placeholder(40), "no events recorded");
    state.run = None;
    let failed = |run_id: &str| {
        PipeMessage::new(
            PipeSource::Plugin(yinz_tile::prelude::PluginId::new(0)),
            "agent_run_detail",
            &None,
            &Some(BTreeMap::from([
                ("error".to_owned(), "store unavailable".to_owned()),
                ("run_id".to_owned(), run_id.to_owned()),
            ])),
            false,
        )
    };
    // About a run the cursor is not on: another row's answer, and not this one's.
    assert!(!state.fixture_pipe(failed("claude:somewhere-else")));
    assert_eq!(state.history_placeholder(40), "loading…");
    assert!(state.fixture_pipe(failed("claude:session-1")));
    assert_eq!(
        state.history_placeholder(40),
        "history unavailable: store unavailable"
    );
    // agent says it again on every snapshot, which is not news.
    assert!(!state.fixture_pipe(failed("claude:session-1")));
    assert_eq!(state.history_placeholder(12), "too narrow for history");
    state.set_rows(unfocused(strip()));
    assert_eq!(state.history_placeholder(40), "nothing to show yet");
}

/// Why a history is unavailable is the end of its sentence, and a half of the width a
/// panel is drawn at cut it off there: the placeholder wraps over the half's rows instead.
#[test]
fn an_unavailable_history_is_wrapped_whole() {
    let mut state = State::default();
    state.set_rows(unfocused(archive()));
    state.selected = 1;
    assert!(state.fixture_pipe(PipeMessage::new(
        PipeSource::Plugin(yinz_tile::prelude::PluginId::new(0)),
        "agent_run_detail",
        &None,
        &Some(BTreeMap::from([
            (
                "error".to_owned(),
                "the run is no longer in the archive".to_owned()
            ),
            ("run_id".to_owned(), "claude:session-1".to_owned()),
        ])),
        false,
    )));
    for (cols, said) in [
        (
            24,
            "history unavailable: the run is no longer in the archive",
        ),
        (
            34,
            "history unavailable: the run is no longer in the archive",
        ),
        (21, "too narrow for history"),
        (12, "too narrow for history"),
    ] {
        let lines = state.placeholder_lines(cols);
        assert!(
            lines.iter().all(|line| line.width() <= cols - 2),
            "{lines:?} at {cols}"
        );
        assert_eq!(lines.join(" "), said, "{cols}");
    }
    state.render(11, 24);
    assert!(state.last_history_body_rows >= state.placeholder_lines(24).len());
}

/// The focus is drawn as a glyph as well as a colour, and a dimmed rule keeps its waiting
/// `!` in the error colour rather than dimming it with the rest.
#[test]
fn the_focus_markers_are_glyphs_as_well_as_colours() {
    let focused = divider_text("3 · claude !", 0, 0, true, 30);
    assert!(focused.content().starts_with("─ ▸ 3 · claude !"));
    let unfocused = divider_text("3 · claude !", 0, 0, false, 30);
    assert!(unfocused.content().starts_with("─   3 · claude !"));
    let at = unfocused
        .content()
        .chars()
        .position(|ch| ch == '!')
        .unwrap();
    // The levels `Text` keeps its dim and error ranges at.
    let (dim, error) = (&unfocused.indices()[4], &unfocused.indices()[6]);
    assert!(!dim.contains(&at));
    assert!(dim.contains(&(at - 1)) && dim.contains(&(at + 1)));
    assert!(error.contains(&at));

    let row = agent(3, "  3  claude  busy");
    let columns = Columns::default();
    assert!(list_text(&row, true, true, &columns, 0)
        .content()
        .starts_with('>'));
    assert!(list_text(&row, true, false, &columns, 0)
        .content()
        .starts_with('·'));
    assert!(list_text(&row, false, false, &columns, 0)
        .content()
        .starts_with(' '));
}

/// The `!` on the divider is the row's own, which knows the agent was answered - not the
/// stored summary's, which goes on saying it is waiting.
#[test]
fn the_divider_marks_a_wait_only_while_the_row_does() {
    let mut state = State::default();
    state.set_rows(with_summary(
        strip(),
        3,
        "Waiting on a permission prompt.",
        true,
    ));
    let label = state.history_lines(38).label;
    assert!(!label.contains('!'), "{label}");
}

/// No label reaches the terminal carrying a character that moves its cursor.
#[test]
fn a_label_cannot_move_the_terminal_cursor() {
    let line = divider_line("edit: a\nb\u{1b}[2J", 0, 0, 40);
    assert!(!line.chars().any(char::is_control), "{line:?}");
    assert!(line.contains("edit: a b"), "{line:?}");
}

/// A hit's cell opens on the word that matched, where the narrow cell can show it.
#[test]
fn a_hit_opens_on_the_word_that_matched() {
    let cell = hit_first(
        "lorem ipsum dolor sit amet consectetur the needle is here",
        "needle",
    );
    assert!(cell.starts_with('…'), "{cell}");
    let at = cell.find("needle").unwrap();
    assert!(cell[..at].chars().count() <= 9, "{cell}");
    assert_eq!(
        hit_first("the needle up front", "needle"),
        "the needle up front"
    );
    assert_eq!(hit_first("nothing matches", "needle"), "nothing matches");
    let folded = hit_first("Case Folded And Then NEEDLE", "\"needle\"");
    assert!(
        folded.starts_with('…') && folded.ends_with("NEEDLE"),
        "{folded}"
    );
}

/// `D` and `F` open the New agent modal on the agent under the cursor - named, so the modal
/// has an agent to copy - with the panel left up under it, so an Esc in the modal comes
/// back to this list. Nothing on a run.
#[test]
fn d_and_f_open_the_new_agent_modal_on_the_agent_under_the_cursor() {
    let fork = pane_action_configuration(7, true);
    assert_eq!(
        fork.get("caller_terminal_pane_id").map(String::as_str),
        Some("7")
    );
    assert_eq!(fork.get("fork_focused").map(String::as_str), Some("true"));
    assert!(!fork.contains_key("duplicate_focused"));
    assert_eq!(
        pane_action_configuration(7, false)
            .get("duplicate_focused")
            .map(String::as_str),
        Some("true")
    );

    let mut state = State::default();
    let mut rows = strip();
    rows.extend(archive());
    state.set_rows(unfocused(rows));
    state.selected = 7;
    assert!(!state.on_key(KeyWithModifier::new(BareKey::Char('D'))));
    assert!(state.new_agent_launches.is_empty(), "a run has `r`");
    assert_eq!(state.panel_hides, 0);
    state.selected = 3;
    state.on_key(KeyWithModifier::new(BareKey::Char('F')).with_shift_modifier());
    assert_eq!(
        state.new_agent_launches,
        [pane_action_configuration(7, true)]
    );
    assert_eq!(state.panel_hides, 0, "the panel stays up under the modal");
}

/// A click on a fold points the reader's cursor at it, and a second one opens it. A prose
/// reader has nothing to point at and ignores clicks, as it always did.
#[test]
fn a_click_on_a_fold_points_at_it_and_a_second_opens_it() {
    let mut state = reading(&task_notification());
    let starts = state.reader_starts().to_vec();
    // In pane rows: the tab bar and the reader's divider are above the body.
    let line = (starts[5] + 2) as isize;
    assert!(state.on_mouse(Mouse::LeftClick((line) - 1, 3)));
    let reader = state.reading.as_ref().expect("a reader");
    assert_eq!(reader.cursor, 5);
    assert!(!reader.sections[5].open);
    assert!(state.on_mouse(Mouse::LeftClick((line) - 1, 3)));
    assert!(state.reading.as_ref().expect("a reader").sections[5].open);

    // The open fold's body is what is being read, not its target: a click there - or the
    // press a drag to copy from it starts with - used to shut it and wind the reader back.
    state.render(20, 50);
    let starts = state.reader_starts().to_vec();
    let end = starts.get(6).copied().unwrap_or(state.reader_lines().len());
    assert!(end > starts[5] + 1, "the open fold has a body");
    let scroll = state.reading.as_ref().expect("a reader").scroll;
    let body = (starts[5] + 1 - scroll + 2) as isize;
    assert!(!state.on_mouse(Mouse::LeftClick((body) - 1, 3)));
    assert!(state.reading.as_ref().expect("a reader").sections[5].open);

    let mut prose = reading(&"word ".repeat(200));
    assert!(!prose.on_mouse(Mouse::LeftClick((3) - 1, 3)));
}

// -- the key sheet ---------------------------------------------------------

/// `?` puts the key sheet up, and `Esc`, `q` or `?` take it down again - `Esc` the sheet,
/// not the panel.
#[test]
fn question_mark_opens_the_key_sheet_and_esc_q_or_question_mark_shut_it() {
    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    state.render(20, 60);
    for shut in [BareKey::Esc, BareKey::Char('q'), BareKey::Char('?')] {
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('?'))));
        assert!(state.key_sheet_scroll.is_some());
        state.render(20, 60);
        assert_eq!(state.footer_hint().as_deref(), Some("Esc/? close"));
        assert!(state.on_key(KeyWithModifier::new(shut)));
        assert!(state.key_sheet_scroll.is_none(), "{shut:?}");
        assert_eq!(
            state.panel_hides, 0,
            "{shut:?} shut the sheet, not the panel"
        );
    }
    // Some terminals report the Shift the `?` took.
    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('?')).with_shift_modifier()));
    assert!(state.key_sheet_scroll.is_some());
    // On the bar, the sheet is the bar's.
    state.key_sheet_scroll = None;
    state.tabs_focused = true;
    state.bar_focused = vec![state.own_client_id];
    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('?'))));
    assert!(state.tabs_focused, "the bar keeps the keys under it");
    assert_eq!(state.key_sheet()[0].0, "←/→");
}

/// The sheet names every key the list answers - and none it does not on this row - and
/// scrolls when it is taller than the pane, without steering anything under it.
#[test]
fn the_key_sheet_lists_what_the_list_answers_and_scrolls_when_it_is_taller() {
    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    state.render(20, 60);
    let keys: Vec<String> = state
        .key_sheet()
        .into_iter()
        .map(|(keys, _)| keys)
        .collect();
    for key in [
        "↑/↓ j/k",
        "Enter",
        "click",
        "s",
        "p",
        "D / F",
        "w",
        "/",
        "PgUp/PgDn",
        "wheel",
        "Tab",
        "Shift+Tab",
        "Ctrl-r",
        "Esc",
        "Ctrl e o",
        "?",
    ] {
        assert!(keys.iter().any(|held| held == key), "{key} in {keys:?}");
    }
    assert!(!keys.iter().any(|held| held == "r"), "r is a run's key");

    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('?'))));
    state.render(7, 60);
    assert_eq!(
        state.footer_hint().as_deref(),
        Some("Esc/? close · j/k scroll")
    );
    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('j'))));
    state.render(7, 60);
    assert_eq!(state.key_sheet_scroll, Some(1));
    for _ in 0..50 {
        state.on_key(KeyWithModifier::new(BareKey::Down));
    }
    state.render(7, 60);
    assert_eq!(state.key_sheet_scroll, Some(state.last_sheet_max_scroll));
    assert!(!state.on_key(KeyWithModifier::new(BareKey::Char('j'))));
    assert!(state.on_key(KeyWithModifier::new(BareKey::PageUp)));
    assert_eq!(state.selected, 2, "and walked no list");
}

/// An empty list has no row to walk, act on or click and no history half to scroll, so
/// its sheet lists the keys that lead out of it - the same ones its footer offers.
#[test]
fn the_key_sheet_on_an_empty_list_lists_no_row_or_history_keys() {
    for view in ["plain", "waiting", "no matches"] {
        // Every one of these is reached from a strip; before the first there is no `Tab`
        // row, which the next test covers.
        let mut state = State {
            strip_seen: true,
            ..Default::default()
        };
        match view {
            "plain" => {
                state.set_rows(unfocused(Vec::new()));
            },
            "waiting" => {
                state.set_rows(unfocused(strip()));
                assert!(state.toggle_waiting());
            },
            _ => {
                state.set_rows(unfocused(strip()));
                let reply = search_reply(&mut state, &[]);
                assert!(state.apply_search_result(reply));
            },
        }
        state.render(20, 60);
        assert!(state.rows.is_empty(), "{view}");
        let sheet = state.key_sheet();
        let keys: Vec<&str> = sheet.iter().map(|(keys, _)| keys.as_str()).collect();
        for key in [
            "↑/↓ j/k",
            "Enter",
            "click",
            "PgUp/PgDn",
            "wheel",
            "s",
            "p",
            "r",
            "D / F",
        ] {
            assert!(!keys.contains(&key), "{view}: {key} in {keys:?}");
        }
        for key in [
            "w",
            "/",
            "Tab",
            "Shift+Tab",
            "Ctrl-r",
            "Esc",
            "Ctrl e o",
            "?",
        ] {
            assert!(keys.contains(&key), "{view}: {key} in {keys:?}");
        }
        assert_eq!(keys.contains(&"c"), view != "plain", "{view}");
        assert_eq!(
            sheet
                .iter()
                .find(|(keys, _)| keys == "Tab")
                .map(|(_, what)| what.as_str()),
            Some("to the panel tab bar"),
            "{view}"
        );
        for key in [BareKey::Enter, BareKey::PageUp, BareKey::PageDown] {
            assert!(!state.on_key(KeyWithModifier::new(key)), "{view}: {key:?}");
        }
        assert_eq!(state.panel_hides, 0, "{view}");
    }
}

/// The sheet agrees with the empty list's footer about `Tab`: not before agent's first
/// strip, when there may be no conversation for it to reach, and from that strip on.
#[test]
fn the_key_sheet_offers_tab_to_the_conversation_only_once_agent_has_answered() {
    let mut state = State::default();
    state.render(20, 60);
    assert!(!state.footer_hint().unwrap().contains("Tab bar"));
    let before = state.key_sheet();
    let names: Vec<&str> = before.iter().map(|(keys, _)| keys.as_str()).collect();
    assert!(!names.contains(&"Tab"), "{names:?}");
    for key in ["Shift+Tab", "/", "w", "Esc"] {
        assert!(names.contains(&key), "{key} in {names:?}");
    }
    let empty = serde_json::to_string(&unfocused(Vec::new())).expect("encoded");
    assert!(state.fixture_pipe(PipeMessage::new(
        PipeSource::Plugin(yinz_tile::prelude::PluginId::new(0)),
        "agent_strip",
        &Some(empty),
        &None,
        false,
    )));
    state.render(20, 60);
    assert!(state.footer_hint().unwrap().contains("Tab bar"));
    assert_eq!(
        state
            .key_sheet()
            .iter()
            .find(|(keys, _)| keys == "Tab")
            .map(|(_, what)| what.as_str()),
        Some("to the panel tab bar")
    );
}

/// The ring works through the sheet, and the sheet then lists the stop the keys went to.
/// Any key it has no use for shuts it and does what it always does, a click shuts it, and a
/// `?` typed into a box is a character.
#[test]
fn keys_the_sheet_does_not_answer_shut_it_and_do_their_job() {
    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    state.render(20, 60);
    state.on_key(KeyWithModifier::new(BareKey::Char('?')));
    assert!(state.on_key(KeyWithModifier::new(BareKey::Tab)));
    assert_eq!(state.focus, Focus::History);
    assert!(state.key_sheet_scroll.is_some(), "the sheet stays up");
    assert!(state
        .key_sheet()
        .iter()
        .any(|(keys, what)| keys == "Enter" && what.contains("read")));

    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('w'))));
    assert!(state.key_sheet_scroll.is_none());
    assert!(state.waiting_base.is_some(), "and the w did its job");
    state.clear_waiting();

    state.begin_search();
    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('?'))));
    assert_eq!(state.search_query, "?");
    assert!(state.key_sheet_scroll.is_none());
    state.clear_search();

    state.render(20, 60);
    state.on_key(KeyWithModifier::new(BareKey::Char('?')));
    assert!(state.on_mouse(Mouse::LeftClick((4) - 1, 0)));
    assert!(state.key_sheet_scroll.is_none());
    assert_eq!(state.selected, 2, "the click did nothing under the sheet");

    // An armed question stays armed under the sheet, which says how to answer it.
    let mut state = State::default();
    state.set_rows(unfocused(archive()));
    state.selected = 1;
    state.on_key(KeyWithModifier::new(BareKey::Char('r')));
    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('?'))));
    assert!(state.confirming_resume.is_some());
    assert_eq!(state.key_sheet()[0].0, "y");
    state.on_key(KeyWithModifier::new(BareKey::Esc));
    assert!(state.confirming_resume.is_some(), "Esc shut the sheet");

    // Every surface has a sheet of its own, even one with nothing listed on it yet - on
    // Todos with no agent to ask, only the key that finds one.
}

/// What a key sheet row says after the keys' column, `key_width` wide: the description's
/// piece on that row, if it has one.
fn described(line: &str, key_width: usize) -> String {
    use unicode_width::UnicodeWidthChar;
    let mut columns = 0;
    line.chars()
        .skip_while(|ch| {
            let skip = columns < 1 + key_width + 2;
            columns += ch.width().unwrap_or(0);
            skip
        })
        .collect()
}

/// Whether `pieces` are `what` wrapped: every word of it, in order, with only the spaces
/// between them moved - or, where a word was too wide for the column, only the word broken.
fn rewraps(pieces: &[String], what: &str) -> bool {
    let words = what.split_whitespace().collect::<Vec<_>>().join(" ");
    let squeezed = |text: &str| text.split_whitespace().collect::<String>();
    pieces.join(" ") == words || squeezed(&pieces.concat()) == squeezed(what)
}

/// The sheet's descriptions line up whatever the keys' widths, and a narrow pane wraps
/// both columns onto more rows rather than cutting either or overflowing.
#[test]
fn the_key_sheet_lines_its_descriptions_up() {
    let short = key_sheet_lines("j/k", "the next fold", 9, 40);
    let long = key_sheet_lines("Shift+Tab", "to the tab bar", 9, 40);
    assert_eq!((short.len(), long.len()), (1, 1));
    assert_eq!(short[0].0.find("the next"), long[0].0.find("to the"));
    assert_eq!(short[0].1, 4, "the margin and the keys");

    let narrow = key_sheet_lines("Shift+Tab", "to the tab bar", 5, 12);
    assert!(narrow.len() > 1, "{narrow:?}");
    assert!(
        narrow.iter().all(|(line, _)| line.width() <= 12),
        "{narrow:?}"
    );
    let pieces: Vec<String> = narrow
        .iter()
        .map(|(line, _)| described(line, 5))
        .filter(|piece| !piece.is_empty())
        .collect();
    assert_eq!(pieces.join(" "), "to the tab bar", "{narrow:?}");
    assert!(
        narrow.iter().skip(2).all(|(_, keys)| *keys == 0),
        "no keys drawn once the keys have ended: {narrow:?}"
    );
}

/// At the width a panel is usually drawn, every sheet wraps its descriptions under their
/// column rather than cutting them: every word of every entry is on screen, and no row is
/// wider than the pane.
#[test]
fn the_key_sheet_wraps_rather_than_cuts_at_the_default_width() {
    let mut agents = State::default();
    agents.set_rows(unfocused(strip()));
    for mut state in [agents] {
        state.key_sheet_scroll = Some(0);
        state.render(39, 24);
        let sheet = state.key_sheet();
        let key_width = sheet
            .iter()
            .map(|(keys, _)| keys.width())
            .max()
            .unwrap_or(0)
            .min(23 / 2);
        for (keys, what) in &sheet {
            let lines = key_sheet_lines(keys, what, key_width, 24);
            assert!(
                lines.iter().all(|(line, _)| line.width() <= 24),
                "Agents: {lines:?}"
            );
            let pieces: Vec<String> = lines
                .iter()
                .map(|(line, _)| described(line, key_width))
                .filter(|piece| !piece.is_empty())
                .collect();
            assert!(rewraps(&pieces, what), "Agents {keys}: {pieces:?}");
            if keys == "x" {
                assert!(pieces.join(" ").ends_with("(asks first)"), "{pieces:?}");
            }
        }
        assert_eq!(
            state.key_sheet_display(24).len(),
            sheet
                .iter()
                .map(|(keys, what)| key_sheet_lines(keys, what, key_width, 24).len())
                .sum::<usize>(),
            "Agents"
        );
    }

    // The footer counts the sheet in the rows it wraps to, and so does its scroll.
    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('?'))));
    state.render(11, 24);
    assert_eq!(
        state.footer_hint().as_deref(),
        Some("Esc/? close · j/k scroll")
    );
    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('j'))));
    assert_eq!(state.key_sheet_scroll, Some(1));
    for _ in 0..100 {
        state.on_key(KeyWithModifier::new(BareKey::Down));
    }
    state.render(11, 24);
    assert_eq!(
        state.last_sheet_max_scroll,
        state.key_sheet_display(24).len() - 10,
        "the last wrapped row can be scrolled onto the screen"
    );
    assert_eq!(state.key_sheet_scroll, Some(state.last_sheet_max_scroll));

    // No `? keys` on a row where `?` shuts the sheet, at any width.
    for cols in 1..=30 {
        state.render(7, cols);
        let footer = state.footer_hint().unwrap();
        assert!(!footer.contains("? keys"), "{footer:?} at {cols}");
    }
    state.render(7, 22);
    assert_eq!(state.footer_hint().as_deref(), Some("Esc/? close"));
    state.render(7, 24);
    assert_eq!(
        state.footer_hint().as_deref(),
        Some("Esc/? close · j/k scroll")
    );
}

#[test]
fn a_delayed_launcher_hide_cannot_reverse_a_later_panel_reveal() {
    let mut state = State::default();
    state.set_rows(unfocused(strip()));
    state.selected = 2;
    state.own_pane_changed(true, true);
    state.hide_the_panel();
    state.own_pane_changed(false, false);
    state.own_pane_changed(true, true);
    let hides = state.panel_hides;
    // Deliver the old launcher's asynchronous hide only after the next visit starts.
    state.pipe(PipeMessage::new(
        PipeSource::Plugin(PluginId::new(99)),
        "agent_strip_hide",
        &None,
        &None,
        false,
    ));
    assert_eq!(
        state.panel_hides, hides,
        "a completed dismissal cannot hide the new visit"
    );
    assert!(state.pane_shown);
    assert!(state.keys_here());
    assert_eq!(state.selected, 2);
}
