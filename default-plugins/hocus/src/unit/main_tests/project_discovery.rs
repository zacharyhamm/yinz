use super::*;

#[test]
fn an_agent_folder_becomes_a_project_on_the_poll() {
    let mut state = state_with_tab(9);
    poll(&mut state, vec![agent_in(3, "/work/hocus", 9)]);

    assert_eq!(state.projects["hocus"].root, "/work/hocus");
    assert!(state.projects["hocus"].discovered);
}

/// A folder Git does not claim is not a project, and gets no tab.
#[test]
fn an_agent_outside_a_repository_becomes_no_project() {
    let mut state = state_with_tab(9);
    poll_with(&mut state, vec![agent_in(3, "/work/notes", 9)], folder);

    assert!(state.projects.is_empty());
    assert!(state.auto_tab_projects.is_empty());
    assert!(state.active_project.is_none());
}

/// Nor does a project root above it lend it one: an agent that is not in a
/// checkout is not working on a project.
#[test]
fn an_agent_outside_a_repository_joins_no_project_it_sits_under() {
    let mut state = state_with_tab(9);
    state.projects.insert(
        "hocus".to_owned(),
        projects::validate("hocus", "/work/hocus").unwrap(),
    );
    poll_with(
        &mut state,
        vec![agent_in(3, "/work/hocus/notes", 9)],
        folder,
    );

    assert!(state.pane_projects().is_empty());
    assert!(state.auto_tab_projects.is_empty());
}

#[test]
fn a_shell_folder_becomes_no_project() {
    let mut state = state_with_tab(9);
    let shell = agents::AgentPane {
        title: "zsh".to_owned(),
        command: Some("zsh".to_owned()),
        ..agent_in(4, "/work/other", 9)
    };
    poll(&mut state, vec![shell]);

    assert!(state.projects.is_empty());
}

#[test]
fn a_tab_whose_agents_share_a_project_is_assigned_automatically() {
    let mut state = state_with_tab(9);
    poll(
        &mut state,
        vec![agent_in(3, "/work/hocus", 9), agent_in(4, "/work/hocus", 9)],
    );

    assert_eq!(state.auto_tab_projects[&9], "hocus");
    assert!(
        state.tab_projects.is_empty(),
        "nothing was assigned by hand"
    );
    assert_eq!(state.active_project.as_deref(), Some("hocus"));
}

#[test]
fn a_tab_with_agents_from_two_projects_gets_no_assignment() {
    let mut state = state_with_tab(9);
    poll(
        &mut state,
        vec![
            agent_in(3, "/work/hocus", 9),
            agent_in(4, "/other/pocus", 9),
        ],
    );

    assert_eq!(state.projects.len(), 2);
    assert!(state.auto_tab_projects.is_empty());
    assert!(state.active_project.is_none());
}

#[test]
fn an_explicit_assignment_wins_over_the_automatic_one() {
    let mut state = state_with_tab(9);
    state.run_local(&agent_call(
        "project_assign",
        r#"{"name":"named","root":"/work/named"}"#,
    ));
    poll(&mut state, vec![agent_in(3, "/work/hocus", 9)]);

    assert_eq!(state.tab_project(9), Some("named"));
    assert_eq!(state.active_project.as_deref(), Some("named"));
    assert!(
        state.auto_tab_projects.is_empty(),
        "an assigned tab is left alone",
    );
}

/// hocus is a global pane, so it may never learn which tab is active. The
/// focused agent is the answer either way.
#[test]
fn the_focused_agent_names_the_project_with_no_tabs_at_all() {
    let mut state = idle_state();
    poll_focused(&mut state, vec![agent_in(3, "/work/hocus", 9)], Some(3));

    assert!(state.tabs.is_empty(), "no TabUpdate ever arrived");
    assert_eq!(state.active_project.as_deref(), Some("hocus"));
}

#[test]
fn focusing_another_agent_moves_the_active_project() {
    let mut state = idle_state();
    let panes = vec![
        agent_in(3, "/work/hocus", 9),
        agent_in(4, "/other/pocus", 9),
    ];
    poll_focused(&mut state, panes.clone(), Some(3));
    assert_eq!(state.active_project.as_deref(), Some("hocus"));

    poll_focused(&mut state, panes, Some(4));
    assert_eq!(state.active_project.as_deref(), Some("pocus"));
}

/// The sidebar, a shell, an editor — none of them is what the user is working
/// on, and the one they asked about must survive them.
#[test]
fn focus_on_a_pane_with_no_agent_leaves_the_project_alone() {
    let mut state = idle_state();
    let shell = agents::AgentPane {
        title: "zsh".to_owned(),
        command: Some("zsh".to_owned()),
        ..agent_in(4, "/work/hocus", 9)
    };
    let panes = vec![agent_in(3, "/work/hocus", 9), shell];
    poll_focused(&mut state, panes.clone(), Some(3));
    assert_eq!(state.active_project.as_deref(), Some("hocus"));

    poll_focused(&mut state, panes.clone(), Some(4));
    assert_eq!(state.active_project.as_deref(), Some("hocus"), "the shell");

    poll_focused(&mut state, panes, None);
    assert_eq!(
        state.active_project.as_deref(),
        Some("hocus"),
        "the sidebar"
    );
}

#[test]
fn the_focused_agent_closing_drops_back_to_the_tab() {
    let mut state = state_with_tab(9);
    poll_focused(&mut state, vec![agent_in(3, "/work/hocus", 9)], Some(3));
    assert_eq!(state.last_focused_agent, Some(3));

    poll_focused(&mut state, vec![agent_in(4, "/other/pocus", 9)], None);
    assert_eq!(state.last_focused_agent, None);
    assert_eq!(state.active_project.as_deref(), Some("pocus"), "by tab");
}

/// Same precedence `tab_project` sets: what the user said by hand outranks
/// what was worked out from a checkout.
#[test]
fn an_explicit_assignment_wins_over_the_focused_agents_folder() {
    let mut state = state_with_tab(9);
    state.run_local(&agent_call(
        "project_assign",
        r#"{"name":"named","root":"/work/named"}"#,
    ));
    poll_focused(&mut state, vec![agent_in(3, "/work/hocus", 9)], Some(3));

    assert_eq!(state.active_project.as_deref(), Some("named"));
}

#[test]
fn a_closed_tab_drops_its_automatic_assignment() {
    let mut state = state_with_tab(9);
    poll(&mut state, vec![agent_in(3, "/work/hocus", 9)]);
    assert_eq!(state.auto_tab_projects[&9], "hocus");

    state.on_tabs(vec![TabInfo {
        active: true,
        tab_id: 10,
        ..TabInfo::default()
    }]);
    assert!(state.auto_tab_projects.is_empty());
    assert!(state.active_project.is_none());
}

#[test]
fn naming_a_discovered_project_leaves_one_project_on_the_root() {
    let mut state = state_with_tab(9);
    poll(&mut state, vec![agent_in(3, "/work/hocus", 9)]);
    assert_eq!(state.active_project.as_deref(), Some("hocus"));

    let result = state.run_local(&agent_call(
        "project_assign",
        r#"{"name":"pocus","root":"/work/hocus"}"#,
    ));

    assert_eq!(result, Some("assigned tab 9 to project pocus".to_owned()));
    assert_eq!(state.projects.keys().collect::<Vec<_>>(), vec!["pocus"]);
    assert!(!state.projects["pocus"].discovered);
    assert_eq!(state.active_project.as_deref(), Some("pocus"));
}

/// The poll request and reply cross the worker boundary as JSON; a field
/// that stops round-tripping silently kills the whole poll.
#[test]
fn poll_reply_round_trips_through_json() {
    let mut activity = agents::Activity::new();
    activity.insert(7, (0xdead_beef, 3));
    let reply = PollReply {
        panes: Some(vec![agents::AgentPane {
            pane_id: 7,
            title: "claude".to_owned(),
            command: Some("claude --resume".to_owned()),
            status: agents::AgentStatus::Running,
            tab: Some((2, "work".to_owned())),
            cwd: Some("/tmp/work".to_owned()),
            host: None,
            detected: None,
        }]),
        activity,
        manual_kinds: BTreeMap::new(),
        repositories: BTreeMap::from([(
            "/tmp/work".to_owned(),
            projects::Repository {
                root: "/tmp/work".to_owned(),
                remote: Some("git@example.com:work.git".to_owned()),
                worktree_status: Some(projects::WorktreeStatus::Linked),
                common_dir: Some("/tmp/main/.git".to_owned()),
                branch: Some("topic".to_owned()),
            },
        )]),
        focused: Some(7),
        summaries: agents::Summaries::new(),
    };
    let json = serde_json::to_string(&reply).expect("serialize");
    let back: PollReply = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.panes, reply.panes);
    assert_eq!(back.activity, reply.activity);
    assert_eq!(back.repositories, reply.repositories);
    assert_eq!(back.focused, reply.focused);
}
