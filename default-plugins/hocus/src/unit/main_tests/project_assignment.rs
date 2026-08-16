use super::*;

// -- projects -----------------------------------------------------------

#[test]
fn project_assignment_remembers_the_root_and_uses_the_active_tab() {
    let mut state = idle_state();
    state.tabs = vec![TabInfo {
        active: true,
        tab_id: 9,
        ..TabInfo::default()
    }];
    let result = state.run_local(&agent_call(
        "project_assign",
        r#"{"name":"hocus","root":"/work/hocus"}"#,
    ));

    assert_eq!(result, Some("assigned tab 9 to project hocus".to_owned()));
    assert_eq!(state.projects["hocus"].root, "/work/hocus");
    assert_eq!(state.tab_projects[&9], "hocus");
    assert_eq!(state.active_project.as_deref(), Some("hocus"));
}

#[test]
fn an_unassigned_active_tab_clears_the_project_context() {
    let mut state = idle_state();
    state.tab_projects.insert(9, "hocus".to_owned());
    state.on_tabs(vec![TabInfo {
        active: true,
        tab_id: 9,
        ..TabInfo::default()
    }]);
    assert_eq!(state.active_project.as_deref(), Some("hocus"));

    state.on_tabs(vec![TabInfo {
        active: true,
        tab_id: 10,
        ..TabInfo::default()
    }]);
    assert!(state.active_project.is_none());
    assert!(
        state.tab_projects.is_empty(),
        "closed tab assignment was removed"
    );
}

#[test]
fn project_agent_lists_use_pane_folders() {
    let mut state = state_with_panes();
    state.projects.insert(
        "hocus".to_owned(),
        projects::validate("hocus", "/work/hocus").unwrap(),
    );
    state.panes[0].cwd = Some("/work/hocus/src".to_owned());
    state.panes[1].cwd = Some("/work/other".to_owned());
    state.repositories = ["/work/hocus/src", "/work/other"]
        .into_iter()
        .map(|cwd| (cwd.to_owned(), checkout(cwd)))
        .collect();
    state.manual_kinds.insert(4, "reviewer".to_owned());

    let list = state
        .run_local(&agent_call("agent_list", r#"{"project":"hocus"}"#))
        .unwrap();
    let list: serde_json::Value = serde_json::from_str(&list).unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["pane_id"], 3);
}

#[test]
fn project_agent_lists_include_matching_remote_worktrees() {
    let mut state = state_with_panes();
    let mut project = projects::validate("hocus", "/work/hocus").unwrap();
    project.remote = Some("git@example.com:hocus.git".to_owned());
    project.worktree_status = Some(projects::WorktreeStatus::Primary);
    project.common_dir = Some("/work/hocus/.git".to_owned());
    state.projects.insert("hocus".to_owned(), project);
    state.panes[0].cwd = Some("/worktrees/topic/src".to_owned());
    state.repositories.insert(
        "/worktrees/topic/src".to_owned(),
        projects::Repository {
            root: "/worktrees/topic".to_owned(),
            remote: Some("git@example.com:hocus.git".to_owned()),
            worktree_status: Some(projects::WorktreeStatus::Linked),
            common_dir: Some("/work/hocus/.git".to_owned()),
            branch: Some("topic".to_owned()),
        },
    );

    assert_eq!(
        state.pane_projects().get(&3).map(String::as_str),
        Some("hocus")
    );
}

/// The daemon probes the folder an agent is in, so a `cd` into a
/// subdirectory has no answer of its own until a fresh probe lands. The
/// agent is in the checkout it was in a moment ago, and on screen the wait
/// read as it losing its project and getting it back.
///
/// The *branch* is not borrowed the same way, and that asymmetry is the
/// point: a folder below a checkout is as likely to be a worktree of it — on
/// its own branch — as a subdirectory of it, and nothing in what has been
/// probed says which. The project survives the wait; the branch waits.
#[test]
fn an_agent_in_a_subdirectory_keeps_the_project_of_its_checkout() {
    let mut state = state_with_panes();
    state.projects.insert(
        "hocus".to_owned(),
        projects::validate("hocus", "/work/hocus").unwrap(),
    );
    state.panes[0].cwd = Some("/work/hocus/src/deep".to_owned());
    state
        .repositories
        .insert("/work/hocus".to_owned(), on_branch("/work/hocus", "topic"));

    assert_eq!(
        state.pane_projects().get(&3).map(String::as_str),
        Some("hocus")
    );
    assert!(
        state.pane_branches().get(&3).is_none(),
        "but a branch is never borrowed from a folder above",
    );
}

/// The reported bug. A worktree nested inside the checkout it came from is,
/// to everything that has only the probed folders to go on, the same shape
/// as a subdirectory of that checkout — so the stand-in that keeps a project
/// steady handed this pane the *primary's* branch, and the picker filed a
/// `topic` agent under `main` beside the agent really working there.
#[test]
fn a_worktree_nested_in_its_checkout_never_shows_the_primary_branch() {
    let mut state = state_with_panes();
    state.panes[0].cwd = Some(NESTED_WORKTREE.to_owned());
    state
        .repositories
        .insert("/work/hocus".to_owned(), on_branch("/work/hocus", "main"));

    assert_eq!(
        state.pane_branches().get(&3),
        None,
        "no answer is not the same as the answer from the folder above",
    );

    // And once the daemon has asked about the folder the pane is actually
    // in, the real answer lands.
    state.repositories.insert(
        NESTED_WORKTREE.to_owned(),
        worktree_of(NESTED_WORKTREE, "/work/hocus", "topic"),
    );
    assert_eq!(
        state.pane_branches().get(&3).map(String::as_str),
        Some("topic")
    );
}

/// The narrowing is about the branch alone: a nested worktree is still the
/// same project as the checkout above it, and says so from the first
/// snapshot rather than after a round trip.
#[test]
fn a_nested_worktree_keeps_the_project_of_the_repository_above_it() {
    let mut state = state_with_panes();
    state.projects.insert(
        "hocus".to_owned(),
        projects::validate("hocus", "/work/hocus").unwrap(),
    );
    state.panes[0].cwd = Some(NESTED_WORKTREE.to_owned());
    state
        .repositories
        .insert("/work/hocus".to_owned(), on_branch("/work/hocus", "main"));

    assert_eq!(
        state.pane_projects().get(&3).map(String::as_str),
        Some("hocus")
    );
}

/// Two worktrees of one repository, nested the way this one keeps them:
/// one project header, and the branch headers doing all the work of telling
/// the two agents apart.
#[test]
fn nested_worktree_agents_group_under_their_own_branches() {
    let mut state = idle_state();
    state.set_panes(vec![
        agent_in(3, "/work/hocus", 1),
        agent_in(5, NESTED_WORKTREE, 2),
    ]);
    state.projects.insert(
        "hocus".to_owned(),
        projects::validate("hocus", "/work/hocus").unwrap(),
    );
    state.repositories = BTreeMap::from([
        ("/work/hocus".to_owned(), on_branch("/work/hocus", "main")),
        (
            NESTED_WORKTREE.to_owned(),
            worktree_of(NESTED_WORKTREE, "/work/hocus", "topic"),
        ),
    ]);
    settle(&mut state);

    assert_eq!(
        agent_entry_texts(&state),
        [
            "hocus",
            "  main",
            "  3  codex  running  tab 1",
            "  topic",
            "  5  codex  running  tab 2",
        ]
    );
}

/// The nearest answer above wins, and only on a component boundary.
#[test]
fn a_sibling_folder_with_a_common_prefix_is_not_an_ancestor() {
    let mut state = state_with_panes();
    state.projects.insert(
        "hocus".to_owned(),
        projects::validate("hocus", "/work/hocus").unwrap(),
    );
    state.panes[0].cwd = Some("/work/hocus-old/src".to_owned());
    state
        .repositories
        .insert("/work/hocus".to_owned(), checkout("/work/hocus"));

    assert!(!state.pane_projects().contains_key(&3));
}

/// A project root that merely contains the folder is a proxy for "this is
/// the repository". When Git says otherwise, Git is right.
#[test]
fn git_identity_beats_a_project_root_that_merely_contains_the_folder() {
    let mut state = state_with_panes();
    // The outer folder the user named, which is not a checkout itself.
    state.projects.insert(
        "workspace".to_owned(),
        projects::validate("workspace", "/work").unwrap(),
    );
    let mut inner = projects::validate("hocus", "/elsewhere/hocus").unwrap();
    inner.remote = Some("git@example.com:hocus.git".to_owned());
    inner.worktree_status = Some(projects::WorktreeStatus::Primary);
    inner.common_dir = Some("/elsewhere/hocus/.git".to_owned());
    state.projects.insert("hocus".to_owned(), inner);

    state.panes[0].cwd = Some("/work/checkout".to_owned());
    state.repositories.insert(
        "/work/checkout".to_owned(),
        projects::Repository {
            root: "/work/checkout".to_owned(),
            remote: Some("git@example.com:hocus.git".to_owned()),
            worktree_status: Some(projects::WorktreeStatus::Linked),
            common_dir: Some("/elsewhere/hocus/.git".to_owned()),
            branch: Some("topic".to_owned()),
        },
    );

    assert_eq!(
        state.pane_projects().get(&3).map(String::as_str),
        Some("hocus"),
        "the repository it is a worktree of, not the folder above it"
    );
}

/// `for_repository` gives up when two projects share a remote. Falling
/// through to the containing root answers with the worktree the user named,
/// which beats answering with nothing.
#[test]
fn two_projects_on_one_remote_fall_back_to_the_containing_root() {
    let mut state = state_with_panes();
    for (name, root) in [("main", "/work/main"), ("topic", "/work/topic")] {
        let mut project = projects::validate(name, root).unwrap();
        project.remote = Some("git@example.com:hocus.git".to_owned());
        project.worktree_status = Some(projects::WorktreeStatus::Primary);
        project.common_dir = Some("/work/main/.git".to_owned());
        state.projects.insert(name.to_owned(), project);
    }
    state.panes[0].cwd = Some("/work/topic/src".to_owned());
    state.repositories.insert(
        "/work/topic/src".to_owned(),
        projects::Repository {
            root: "/work/topic".to_owned(),
            remote: Some("git@example.com:hocus.git".to_owned()),
            worktree_status: Some(projects::WorktreeStatus::Linked),
            common_dir: Some("/work/main/.git".to_owned()),
            branch: Some("topic".to_owned()),
        },
    );

    assert_eq!(
        state.pane_projects().get(&3).map(String::as_str),
        Some("topic")
    );
}

/// The case the rename bookkeeping exists for: a tab points at a project a
/// later poll absorbs, and must follow it rather than name nothing.
#[test]
fn absorbed_project_moves_its_tab() {
    let mut state = state_with_tab(9);
    poll(&mut state, vec![agent_in(3, "/work/hocus/src", 9)]);
    assert_eq!(state.active_project.as_deref(), Some("src"));

    poll(
        &mut state,
        vec![
            agent_in(3, "/work/hocus/src", 9),
            agent_in(4, "/work/hocus", 9),
        ],
    );

    assert_eq!(state.projects.keys().collect::<Vec<_>>(), vec!["hocus"]);
    assert_eq!(state.auto_tab_projects[&9], "hocus");
    assert_eq!(state.active_project.as_deref(), Some("hocus"));
}

#[test]
fn forgetting_a_project_clears_what_named_it() {
    let mut state = state_with_tab(9);
    state.run_local(&agent_call(
        "project_assign",
        r#"{"name":"hocus","root":"/work/hocus"}"#,
    ));
    assert_eq!(state.tab_projects[&9], "hocus");

    let result = state.run_local(&agent_call("project_forget", r#"{"name":"hocus"}"#));

    assert_eq!(result, Some("forgot hocus".to_owned()));
    assert!(state.projects.is_empty());
    assert!(state.tab_projects.is_empty(), "the tab named a project");
    assert!(state.active_project.is_none());
}

#[test]
fn forgetting_an_unknown_project_says_what_to_call() {
    let mut state = idle_state();
    let result = state
        .run_local(&agent_call("project_forget", r#"{"name":"nope"}"#))
        .unwrap();

    assert!(result.contains("project_list"), "got {result}");
}

/// Nothing remembers a project that was forgotten, so a poll that still
/// sees an agent in the checkout makes it again. The answer says so.
#[test]
fn forgetting_a_discovered_project_says_a_poll_makes_it_again() {
    let mut state = state_with_tab(9);
    poll(&mut state, vec![agent_in(3, "/work/hocus", 9)]);

    let result = state
        .run_local(&agent_call("project_forget", r#"{"name":"hocus"}"#))
        .unwrap();
    assert!(result.contains("/work/hocus"), "got {result}");
    assert!(state.projects.is_empty());
    assert!(state.auto_tab_projects.is_empty());

    poll(&mut state, vec![agent_in(3, "/work/hocus", 9)]);
    assert_eq!(state.projects["hocus"].root, "/work/hocus");
}

/// The only tool that removes something the user meant to keep, so it stops
/// for a y/n rather than running on the model's say-so.
#[test]
fn forgetting_a_project_waits_for_a_confirmation() {
    let mut state = ready_state();
    state.projects.insert(
        "hocus".to_owned(),
        projects::validate("hocus", "/work/hocus").unwrap(),
    );
    let frame = tool_call_frame("call_1", "project_forget", r#"{"name":"hocus"}"#);
    state.on_chunk("hocus-1", &frame, false, None);
    state.on_chunk("hocus-1", b"", true, None);

    assert!(matches!(state.status, Status::ConfirmingTool));
    assert!(state.projects.contains_key("hocus"), "nothing yet");
    state.confirm_tool();
    assert!(state.projects.is_empty());
}

#[test]
fn an_agent_list_names_the_folder_and_the_project() {
    let mut state = state_with_tab(9);
    poll(&mut state, vec![agent_in(3, "/work/hocus/src", 9)]);

    let list = state.run_local(&agent_call("agent_list", "{}")).unwrap();
    let list: serde_json::Value = serde_json::from_str(&list).unwrap();
    assert_eq!(list[0]["cwd"], "/work/hocus/src");
    assert_eq!(list[0]["project"], "src");
}

#[test]
fn project_agent_spawn_rejects_a_folder_outside_the_root() {
    let mut state = idle_state();
    state.projects.insert(
        "hocus".to_owned(),
        projects::validate("hocus", "/work/hocus").unwrap(),
    );
    assert_eq!(
        state.run_local(&agent_call(
            "agent_spawn",
            r#"{"command":"codex","project":"hocus","cwd":"/work/other"}"#,
        )),
        Some("cwd must be inside project hocus".to_owned()),
    );
}

#[test]
fn a_project_supplies_the_default_agent_folder() {
    let mut state = idle_state();
    state.projects.insert(
        "hocus".to_owned(),
        projects::validate("hocus", "/work/hocus").unwrap(),
    );
    state.active_project = Some("hocus".to_owned());
    let args = tools::Args::parse(r#"{"command":"codex"}"#).unwrap();
    assert_eq!(
        state.agent_cwd(&args).unwrap().as_deref(),
        Some("/work/hocus")
    );
}

#[test]
fn a_project_tab_uses_the_stable_id_from_zellij() {
    let mut state = idle_state();
    let result = state.register_opened_project("hocus", r#"{"opened":true,"tab_id":47}"#);
    assert_eq!(result, r#"{"opened":true,"tab_id":47}"#);
    assert_eq!(state.tab_projects[&47], "hocus");
    assert_eq!(state.active_project.as_deref(), Some("hocus"));
}
