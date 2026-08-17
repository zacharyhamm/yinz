use super::*;

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
    let mut state = state_with_tab(9);
    poll(&mut state, vec![agent_in(3, "/work/hocus", 9)]);
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
    let mut state = state_with_tab(9);
    let panes = vec![agent_in(3, "/work/hocus", 9), agent_in(7, "/work/hocus", 9)];

    poll_focused(&mut state, panes.clone(), Some(7));
    assert_eq!(state.last_strip.focused, Some(7));

    // pane 4 is a shell: the strip says so, even as the active project goes
    // on answering "pane 7"
    poll_focused(&mut state, panes.clone(), Some(4));
    assert_eq!(state.last_strip.focused, Some(4));
    assert_eq!(state.last_focused_agent, Some(7));
}

/// The id the agents pane announces itself with. Any number; what matters is
/// that hocus compares it against the plugin pane the daemon reports focused.
const AGENTS_PANE: u32 = 11;

/// `state_with_panes` - one `codex` agent on pane 3, one shell on pane 4 - with
/// the agents pane known, `selected` under its cursor, and pane 3 holding a
/// summary. `polls` is how long ago the agent was last seen moving, which the
/// inset does not care about and the tests below say so.
fn state_with_summary(selected: Option<u32>, polls: u32) -> State {
    let mut state = state_with_panes();
    state.agents_plugin = Some(AGENTS_PANE);
    state.focused_plugin = Some(AGENTS_PANE);
    state.agents_selected = selected;
    state.activity = agents::Activity::from([(3, (11, polls))]);
    state.summaries = agents::Summaries::from([(
        3,
        agents::Summary {
            text: "Ran the tests.".to_owned(),
            title: "running the tests".to_owned(),
            awaiting: true,
            hash: 11,
        },
    )]);
    state
}

/// The inset is a note about the row the user's cursor is on in the agents
/// pane, which is neither the focused pane nor the sticky `last_focused_agent`.
#[test]
fn the_selected_agents_summary_is_what_the_inset_draws() {
    let state = state_with_summary(Some(3), 2);
    let (pane_id, kind, summary) = state.selected_summary().expect("the selected agent");
    assert_eq!((pane_id, kind), (3, "codex"));
    assert_eq!(summary.text, "Ran the tests.");
    assert_eq!(state.inset_key(), Some((3, 11, true)));
}

/// What is asked for is the *last* thing the daemon made of this agent, so an
/// agent that has gone back to work goes on showing it. The `!` marker in the
/// header says which of the two kinds of stop it was; the pane id says which
/// pane it was about; neither claims to be an account of right now.
#[test]
fn an_agent_back_at_work_still_shows_its_last_summary() {
    let state = state_with_summary(Some(3), 0);
    assert_eq!(state.inset_key(), Some((3, 11, true)));
}

/// Anywhere but the list, the box is not what the user is reading.
#[test]
fn nothing_is_drawn_unless_the_agents_pane_holds_the_focus() {
    // Standing in the agent itself.
    let mut state = state_with_summary(Some(3), 2);
    state.focused_plugin = None;
    state.focused_pane = Some(3);
    assert!(state.selected_summary().is_none());

    // In another plugin pane - hocus itself, most often.
    let mut state = state_with_summary(Some(3), 2);
    state.focused_plugin = Some(AGENTS_PANE + 1);
    assert!(state.selected_summary().is_none());

    // And before the agents pane has said which id it is, there is nothing to
    // compare a focused plugin pane against.
    let mut state = state_with_summary(Some(3), 2);
    state.agents_plugin = None;
    assert!(state.selected_summary().is_none());
}

#[test]
fn nothing_is_drawn_for_a_selection_that_is_no_agent_or_has_nothing_to_say() {
    // A shell, with the agent's summary still in the map beside it.
    assert!(state_with_summary(Some(4), 2).selected_summary().is_none());
    // A strip with nothing selectable on it.
    assert!(state_with_summary(None, 2).selected_summary().is_none());
    // An agent the daemon has never had an answer about.
    let mut state = state_with_summary(Some(3), 2);
    state.summaries.clear();
    assert!(state.selected_summary().is_none());
}

/// The agents pane's cursor arrives on a pipe, and moving it is worth a
/// repaint here: nothing the user typed in this pane changed, but the box did.
#[test]
fn a_selection_message_moves_the_inset_and_repaints() {
    let mut state = state_with_summary(None, 2);
    state.agents_plugin = None;
    state.agents_selected = None;

    // The first message carries the plugin id and no selection yet.
    assert!(!state.pipe(selection(AGENTS_PANE, None)));
    assert_eq!(state.agents_plugin, Some(AGENTS_PANE));
    assert_eq!(state.inset_key(), None);

    // Walking onto the agent brings the box with it.
    assert!(state.pipe(selection(AGENTS_PANE, Some(3))));
    assert_eq!(state.inset_key(), Some((3, 11, true)));

    // And off it again, onto a pane with nothing to show.
    assert!(state.pipe(selection(AGENTS_PANE, Some(4))));
    assert_eq!(state.inset_key(), None);
}

/// One `AGENT_SELECTION` from the agents pane.
fn selection(plugin_id: u32, pane_id: Option<u32>) -> PipeMessage {
    PipeMessage {
        source: PipeSource::Plugin(plugin_id),
        name: wire::AGENT_SELECTION.to_owned(),
        payload: Some(
            serde_json::to_string(&wire::Selection { plugin_id, pane_id }).expect("serialize"),
        ),
        args: BTreeMap::new(),
        is_private: true,
    }
}

/// A push is owed for a focus change alone. Walking from one agent to
/// another leaves every row identical, and the dedupe used to compare only
/// rows - which would have left the cursor behind.
#[test]
fn walking_between_agents_pushes_a_strip_of_identical_rows() {
    let mut state = state_with_tab(9);
    let panes = vec![agent_in(3, "/work/hocus", 9), agent_in(7, "/work/hocus", 9)];
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
