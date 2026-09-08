use super::*;
use hocus_proto::store::Backend;
use zellij_tile::prelude::HostId;
use zellij_tile::prelude::TabId;

/// The strip rows hocus pushes to the agents pane: agents grouped under the
/// project and branch headers, computed from the snapshot it holds.
#[test]
fn the_strip_groups_agents_under_their_headers() {
    let mut state = state_with_panes();
    settle(&mut state);

    assert_eq!(
        agent_entry_texts(&state),
        ["no project", "  no branch", "    3  codex  running"]
    );
    assert!(matches!(
        state.current_strip().rows[2],
        agents::StripRow::Agent { pane_id: 3, .. }
    ));
}

/// Two worktrees of one repository — the case the branch headers exist for.
/// Stepping over the headers is the hocus-agents pane's job and is tested
/// there; hocus's half of the deal is producing rows in this shape.
#[test]
fn worktree_agents_group_under_their_branch_headers() {
    let mut state = state_with_worktrees();
    settle(&mut state);

    assert_eq!(
        agent_entry_texts(&state),
        [
            "no project",
            "  main",
            "    3  codex  running  tab 1",
            "  topic",
            "    5  codex  running  tab 2",
        ]
    );
}

/// Focusing the agents pane asks for the branches to be re-read, so the
/// snapshot that answers has to update the strip. Following the selected
/// pane through the reshuffle is the hocus-agents pane's job.
#[test]
fn a_snapshot_rebuilds_the_strip() {
    let mut state = state_with_worktrees();
    settle(&mut state);

    // Pane 3 turns out to be on `zzz`, which sorts it below pane 5. The
    // strip goes to the agents pane, so hocus itself has nothing to redraw.
    assert!(!state.pipe(worktree_snapshot(7, "zzz")));

    assert_eq!(
        agent_entry_texts(&state),
        [
            "no project",
            "  topic",
            "    5  codex  running  tab 2",
            "  zzz",
            "    3  codex  running  tab 1",
        ]
    );
}

#[test]
fn project_changes_push_the_strip_immediately() {
    let mut state = state_with_tab(TabId::new(9));
    poll(&mut state, vec![agent_in(3, "/work/hocus", TabId::new(9))]);
    assert!(matches!(
        &state.last_strip.rows[0],
        agents::StripRow::Project(name) if name == "hocus"
    ));

    state.run_local(&agent_call(
        "project_assign",
        r#"{"name":"pocus","root":"/work/hocus"}"#,
    ));
    assert!(matches!(
        &state.last_strip.rows[0],
        agents::StripRow::Project(name) if name == "pocus"
    ));

    state.run_local(&agent_call("project_forget", r#"{"name":"pocus"}"#));
    assert!(matches!(
        &state.last_strip.rows[0],
        agents::StripRow::Project(name) if name == "no project"
    ));
}

/// The strip carries the focused pane, raw: the agents pane puts its cursor
/// on the agent the user walked into, and dims it when they walk off onto
/// something that is no agent at all. `last_focused_agent` is sticky and
/// could only ever say the first of those.
#[test]
fn the_strip_carries_the_focused_pane_agent_or_not() {
    let mut state = state_with_tab(TabId::new(9));
    let panes = vec![
        agent_in(3, "/work/hocus", TabId::new(9)),
        agent_in(7, "/work/hocus", TabId::new(9)),
    ];

    poll_focused(&mut state, panes.clone(), Some(7));
    assert_eq!(state.last_strip.focused, Some(7));

    // pane 4 is a shell: the strip says so, even as the active project goes
    // on answering "pane 7"
    poll_focused(&mut state, panes.clone(), Some(4));
    assert_eq!(state.last_strip.focused, Some(4));
    assert_eq!(state.last_focused_agent, Some(7));
}

/// The id the agents pane announces itself with. Any number; what matters is
/// that hocus records it, because it is the only thing that lets hocus place the
/// list beside itself.
const AGENTS_PANE: PluginId = PluginId::new(11);

/// `state_with_panes` - one `codex` agent on pane 3, one shell on pane 4 - with
/// the agents pane known and pane 3 holding a summary. `polls` is how long ago
/// the agent was last seen moving.
fn state_with_summary(polls: u32) -> State {
    let mut state = state_with_panes();
    state.agents_plugin = Some(AGENTS_PANE);
    state.activity = agents::Activity::from([(3, (11, polls))]);
    state.summaries = agents::Summaries::from([(
        3,
        agents::Summary {
            text: "Ran the tests.".to_owned(),
            title: "running the tests".to_owned(),
            awaiting: true,
            hash: 11,
            created_at_ms: None,
        },
    )]);
    state
}

/// The summary box is drawn in the agents pane now, so what hocus owes it is the
/// summary itself - on the same message as the rows, keyed by the pane each row
/// names.
#[test]
fn the_strip_carries_the_summary_of_every_agent_on_it() {
    let state = state_with_summary(2);
    let strip = state.current_strip();
    let summary = strip.summaries.get(&3).expect("pane 3's summary");
    assert_eq!(summary.text, "Ran the tests.");
    assert!(summary.awaiting, "and the claim the `!` marker makes");
}

#[test]
fn completed_runs_are_nested_under_the_archive_and_page_on_demand() {
    let mut state = state_with_panes();
    state.archived_runs.push(agents::AgentRunHeader {
        id: "claude:session-1".to_owned(),
        pane_id: 3,
        origin_run_id: None,
        previous_run_id: None,
        session_id: "session-1".to_owned(),
        kind: "claude".to_owned(),
        cwd: Some("/work/hocus".to_owned()),
        host: None,
        worktree: None,
        started_at_ms: 10,
        ended_at_ms: Some(20),
        outcome: agents::RunOutcome::Completed,
        revision: 4,
        summary: None,
        title: "hook history".to_owned(),
    });
    state.archive_more = true;

    let rows = state.current_strip().rows;
    assert!(matches!(
        &rows[rows.len() - 3],
        agents::StripRow::Project(name) if name == "archive"
    ));
    assert!(matches!(
        &rows[rows.len() - 2],
        agents::StripRow::Run { run_id, text, .. }
            if run_id == "claude:session-1" && text == "claude · hook history"
    ));
    assert!(matches!(rows.last(), Some(agents::StripRow::MoreRuns)));
}

/// One archive header, as `load_more_agent_runs` puts them in `archived_runs`.
fn header(session_id: &str, host: Option<&str>) -> agents::AgentRunHeader {
    agents::AgentRunHeader {
        id: agents::run_id("claude", 3, session_id, 10, &HostId::new("test-host")),
        pane_id: 3,
        origin_run_id: None,
        previous_run_id: None,
        session_id: session_id.to_owned(),
        kind: "claude".to_owned(),
        cwd: Some("/work/hocus".to_owned()),
        host: host.map(str::to_owned),
        worktree: None,
        started_at_ms: 10,
        ended_at_ms: Some(20),
        outcome: agents::RunOutcome::Completed,
        revision: 4,
        summary: None,
        title: "hook history".to_owned(),
    }
}

/// The same run in the store, where a resume is decided from: `archived_runs` is a snapshot
/// of the page that was loaded, and only the record can say that another session has since
/// picked the run back up. Written through the mutations hocus-agentd writes, against the
/// temporary store, so nothing here stands in for the daemon.
fn seed_run(state: &State, session_id: &str, host: Option<&str>, ended: bool) {
    let run = agents::AgentRun {
        id: agents::run_id("claude", 3, session_id, 10, &HostId::new("test-host")),
        pane_id: 3,
        zellij_session_name: "main".to_owned().into(),
        host_id: HostId::new("test-host"),
        origin_run_id: None,
        previous_run_id: None,
        session_id: session_id.to_owned(),
        kind: "claude".to_owned(),
        cwd: Some("/work/hocus".to_owned()),
        host: host.map(str::to_owned),
        worktree: None,
        started_at_ms: 10,
        ended_at_ms: ended.then_some(20),
        outcome: match ended {
            true => agents::RunOutcome::Completed,
            false => agents::RunOutcome::Active,
        },
        revision: 4,
        summarized_revision: 0,
        summary: None,
        entries: Vec::new(),
    };
    state
        .store()
        .apply(agents::rows::run_mutations(&run))
        .expect("seeded the run");
}

/// `r` then `y` in the panel: the row carries a run id and nothing else, so hocus is what
/// turns it into a command. The record it reads back has every part of that.
#[test]
fn an_archived_run_resumes_as_its_own_agent_in_its_own_folder() {
    let state = state_with_panes();
    seed_run(&state, "session-1", None, true);

    let command = state.resume_plan("claude:session-1").expect("planned");
    assert_eq!(command.path, std::path::PathBuf::from("claude"));
    assert!(
        command
            .args
            .starts_with(&["--resume".to_owned(), "session-1".to_owned()]),
        "{:?}",
        command.args
    );
    assert_eq!(command.cwd, Some(std::path::PathBuf::from("/work/hocus")));
}

/// The refusals the panel puts on its divider. Two of them are the run's own doing and one
/// is the archive's - and none of them is silence, because the user answered a confirmation
/// to get here.
#[test]
fn a_run_that_cannot_be_resumed_says_why() {
    let state = state_with_panes();
    // A hook script that reported no session id: the run is keyed by pane and start time,
    // and there is nothing in that to resume.
    seed_run(&state, "", None, true);
    let refused = state
        .resume_plan(&agents::run_id(
            "claude",
            3,
            "",
            10,
            &HostId::new("test-host"),
        ))
        .unwrap_err();
    assert!(refused.contains("no session id"), "{refused}");

    // A VM that is gone, and cannot be rebuilt from what the run recorded.
    seed_run(&state, "session-2", Some("firecracker:vm-1"), true);
    let refused = state.resume_plan("claude:session-2").unwrap_err();
    assert!(refused.contains("vm-1"), "{refused}");

    // And a row this hocus has never loaded and the store has never held.
    let refused = state.resume_plan("claude:never-heard-of-it").unwrap_err();
    assert!(refused.contains("archive"), "{refused}");
}

/// A run can go live between the strip being drawn and the `y` — someone resumed it in
/// another session, or a warp brought its pane here. The archive row is stale by a beat, and
/// honouring it would put two processes on one conversation.
///
/// Both halves of that: a pane in *this* session, which `active_runs` names, and a run the
/// record says is open again, which only the store can say - the cached header a row was
/// drawn from goes on claiming the run ended.
#[test]
fn a_run_that_went_live_under_the_row_is_not_resumed_twice() {
    let mut state = state_with_panes();
    seed_run(&state, "session-1", None, true);
    state.archived_runs.push(header("session-1", None));
    assert!(state.resume_plan("claude:session-1").is_ok());

    state.active_runs.insert(7, header("session-1", None));
    let refused = state.resume_plan("claude:session-1").unwrap_err();
    assert!(refused.contains("pane 7"), "{refused}");

    // The pane is elsewhere, so this session knows nothing about it - but the record does.
    state.active_runs.clear();
    seed_run(&state, "session-1", None, false);
    let refused = state.resume_plan("claude:session-1").unwrap_err();
    assert!(refused.contains("another session"), "{refused}");
}

/// What is carried is the *last* thing the daemon made of an agent, so one that
/// has gone back to work goes on carrying it: the box is an account of a moment,
/// which is why its header names the pane.
#[test]
fn an_agent_back_at_work_still_carries_its_last_summary() {
    assert!(state_with_summary(0)
        .current_strip()
        .summaries
        .contains_key(&3));
}

/// A summary outlives the pane it is about. The box is drawn *for a row*, so one
/// no row names could never be selected and has no business on the wire.
#[test]
fn a_summary_with_no_row_to_sit_under_is_not_sent() {
    let mut state = state_with_summary(2);
    state.summaries.insert(
        99,
        agents::Summary {
            text: "a pane that has since closed".to_owned(),
            title: String::new(),
            awaiting: false,
            hash: 1,
            created_at_ms: None,
        },
    );
    let strip = state.current_strip();
    assert!(strip.summaries.contains_key(&3));
    assert!(!strip.summaries.contains_key(&99), "no row names pane 99");
    // and the shell on pane 4 is no agent, so it has no row either
    assert!(!strip.summaries.contains_key(&4));
}

/// The one thing hocus still wants from `AGENT_SELECTION`: the list's plugin id.
/// It is not worth a repaint here - nothing on this pane's screen comes from it -
/// but it is what `apply_overlay_geometry` needs to place the list.
#[test]
fn a_selection_message_is_how_hocus_learns_the_agent_lists_id() {
    let mut state = state_with_summary(2);
    state.agents_plugin = None;

    assert!(!state.pipe(selection(AGENTS_PANE, None)));
    assert_eq!(state.agents_plugin, Some(AGENTS_PANE));

    // and walking the list is silent here
    assert!(!state.pipe(selection(AGENTS_PANE, Some(3))));
    assert_eq!(state.agents_plugin, Some(AGENTS_PANE));
}

#[test]
fn a_selected_agent_picks_up_its_run_when_the_first_header_arrives() {
    let mut state = state_with_summary(2);
    assert!(!state.pipe(selection(AGENTS_PANE, Some(3))));
    assert_eq!(state.selected_run, None);

    let mut run = agents::AgentRunHeader {
        id: "claude:session-1".to_owned(),
        pane_id: 3,
        origin_run_id: None,
        previous_run_id: None,
        session_id: "session-1".to_owned(),
        kind: "claude".to_owned(),
        cwd: Some("/work/hocus".to_owned()),
        host: None,
        worktree: None,
        started_at_ms: 10,
        ended_at_ms: None,
        outcome: agents::RunOutcome::Active,
        revision: 1,
        summary: None,
        title: "hook history".to_owned(),
    };
    state.merge_run_headers(vec![run.clone()]);
    assert_eq!(state.selected_run.as_deref(), Some("claude:session-1"));

    run.id = "claude:session-2".to_owned();
    run.session_id = "session-2".to_owned();
    state.merge_run_headers(vec![run]);
    assert_eq!(state.selected_run.as_deref(), Some("claude:session-2"));
}

#[test]
fn archive_offset_moves_only_for_completions_before_the_loaded_page() {
    let completed = |id: &str, started_at_ms: i64| agents::AgentRunHeader {
        id: id.to_owned(),
        pane_id: 3,
        origin_run_id: None,
        previous_run_id: None,
        session_id: id.to_owned(),
        kind: "claude".to_owned(),
        cwd: None,
        host: None,
        worktree: None,
        started_at_ms,
        ended_at_ms: Some(200),
        outcome: agents::RunOutcome::Completed,
        revision: 1,
        summary: None,
        title: "run".to_owned(),
    };
    let mut state = State {
        archive_offset: 50,
        archive_loaded_through: Some((100, "middle".to_owned())),
        ..State::default()
    };

    state.merge_run_headers(vec![completed("older", 50)]);
    assert_eq!(state.archive_offset, 50);

    state.merge_run_headers(vec![completed("newer", 150)]);
    assert_eq!(state.archive_offset, 51);

    state.merge_run_headers(vec![completed("z-after-middle", 100)]);
    assert_eq!(state.archive_offset, 51);

    state.merge_run_headers(vec![completed("a-ahead-of-middle", 100)]);
    assert_eq!(state.archive_offset, 52);
}

/// One `AGENT_SELECTION` from the agents pane.
fn selection(plugin_id: zellij_tile::prelude::PluginId, pane_id: Option<u32>) -> PipeMessage {
    PipeMessage {
        source: PipeSource::Plugin(plugin_id),
        name: wire::AGENT_SELECTION.to_owned(),
        payload: Some(
            serde_json::to_string(&wire::Selection {
                plugin_id,
                pane_id,
                run_id: None,
            })
            .expect("serialize"),
        ),
        args: BTreeMap::new(),
        is_private: true,
        pane_id: None,
    }
}

/// A push is owed for a focus change alone. Walking from one agent to
/// another leaves every row identical, and the dedupe used to compare only
/// rows - which would have left the cursor behind.
#[test]
fn walking_between_agents_pushes_a_strip_of_identical_rows() {
    let mut state = state_with_tab(TabId::new(9));
    let panes = vec![
        agent_in(3, "/work/hocus", TabId::new(9)),
        agent_in(7, "/work/hocus", TabId::new(9)),
    ];
    poll_focused(&mut state, panes.clone(), Some(3));
    let rows = state.last_strip.rows.clone();

    poll_focused(&mut state, panes.clone(), Some(7));
    assert_eq!(state.last_strip.rows, rows, "nothing about a row changed");
    assert_eq!(state.last_strip.focused, Some(7), "but the push happened");
}

/// The reported symptom, at the layer the user sees it: walking between two
/// agents must move the cursor and nothing else. Every snapshot replaces
/// `repositories` wholesale, so anything that churns the folders on a focus
/// change takes the branch headers with it.
#[test]
fn walking_between_worktree_agents_keeps_both_branch_headers() {
    let mut state = idle_state();
    settle(&mut state);

    let headers = |state: &State| -> Vec<String> {
        state
            .current_strip()
            .rows
            .into_iter()
            .filter_map(|row| match row {
                agents::StripRow::Branch(text) => Some(text),
                _ => None,
            })
            .collect()
    };

    state.pipe(nested_worktree_snapshot(1, Some(3)));
    let expected = headers(&state);
    assert_eq!(expected, ["  main", "  topic"], "both branches are known");
    let branches = state.pane_branches();

    // Onto the other agent, then onto the sidebar, which is no terminal pane
    // at all and reports no focus.
    for (seq, focused) in [(2, Some(5)), (3, None), (4, Some(3))] {
        state.pipe(nested_worktree_snapshot(seq, focused));
        assert_eq!(
            headers(&state),
            expected,
            "the branch headers moved on a focus change to {focused:?}",
        );
        assert_eq!(state.pane_branches(), branches);
    }

    assert_eq!(
        state.last_strip.focused,
        Some(3),
        "the focus itself did move — this is not a test that nothing happened",
    );
}

#[test]
fn continuations_are_marked_without_changing_history_order_or_keys() {
    let mut state = state_with_panes();
    let mut live = header("C", None);
    live.origin_run_id = Some("claude:A".into());
    live.previous_run_id = Some("claude:B".into());
    live.ended_at_ms = None;
    live.outcome = agents::RunOutcome::Active;
    let mut archived = header("B", None);
    archived.origin_run_id = Some("claude:A".into());
    archived.previous_run_id = Some("claude:A".into());
    archived.started_at_ms = 20;
    // The daemon sends live headers first, then completed runs from the same pane.
    state.merge_run_headers(vec![live, archived, header("A", None)]);
    let rows = state.current_strip().rows;
    assert!(rows.iter().any(|row| matches!(row, agents::StripRow::Agent { pane_id: 3, text, .. } if text.trim_start().starts_with("↳ "))));
    let archive: Vec<_> = rows
        .iter()
        .filter_map(|row| match row {
            agents::StripRow::Run { run_id, text, .. } => Some((run_id.as_str(), text.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(
        archive,
        vec![
            ("claude:B", "↳ claude · hook history"),
            ("claude:A", "claude · hook history")
        ]
    );
}
