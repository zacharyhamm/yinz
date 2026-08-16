use super::*;

/// The strip rows hocus pushes to the agents pane: agents grouped under the
/// project and branch headers, computed from the snapshot it holds.
#[test]
fn the_strip_groups_agents_under_their_headers() {
    let mut state = state_with_panes();
    settle(&mut state);

    assert_eq!(
        agent_entry_texts(&state),
        ["no project", "  no branch", "  3  codex  running"]
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
            "  3  codex  running  tab 1",
            "  topic",
            "  5  codex  running  tab 2",
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
            "  5  codex  running  tab 2",
            "  zzz",
            "  3  codex  running  tab 1",
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
