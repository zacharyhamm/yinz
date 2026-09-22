use super::*;
use zellij_tile::prelude::TabId;

#[test]
fn an_agent_folder_becomes_a_project_on_the_poll() {
    let mut state = state_with_tab(TabId::new(9));
    poll(&mut state, vec![agent_in(3, "/work/hocus", TabId::new(9))]);

    assert_eq!(state.projects["hocus"].root, "/work/hocus");
    assert!(state.projects["hocus"].discovered);
}

/// A folder Git does not claim is not a project, and gets no tab.
#[test]
fn an_agent_outside_a_repository_becomes_no_project() {
    let mut state = state_with_tab(TabId::new(9));
    poll_with(
        &mut state,
        vec![agent_in(3, "/work/notes", TabId::new(9))],
        folder,
    );

    assert!(state.projects.is_empty());
    assert!(state.auto_tab_projects.is_empty());
    assert!(state.active_project.is_none());
}

/// Nor does a project root above it lend it one: an agent that is not in a
/// checkout is not working on a project.
#[test]
fn an_agent_outside_a_repository_joins_no_project_it_sits_under() {
    let mut state = state_with_tab(TabId::new(9));
    state.projects.insert(
        "hocus".to_owned(),
        projects::validate("hocus", "/work/hocus").unwrap(),
    );
    poll_with(
        &mut state,
        vec![agent_in(3, "/work/hocus/notes", TabId::new(9))],
        folder,
    );

    assert!(state.pane_projects().is_empty());
    assert!(state.auto_tab_projects.is_empty());
}

#[test]
fn a_tab_is_labelled_with_the_project_its_agents_are_in() {
    let mut state = state_with_tab(TabId::new(9));
    poll(
        &mut state,
        vec![
            agent_in(3, "/work/hocus", TabId::new(9)),
            agent_in(4, "/work/hocus", TabId::new(9)),
        ],
    );

    assert_eq!(state.tab_project_labels[&TabId::new(9)], "hocus");
}

/// The label names both, where the *assignment* names neither: a tab is a place
/// to work rather than a thing one project owns.
#[test]
fn a_tab_with_two_projects_is_labelled_with_both() {
    let mut state = state_with_tab(TabId::new(9));
    poll(
        &mut state,
        vec![
            agent_in(3, "/work/pocus", TabId::new(9)),
            agent_in(4, "/work/hocus", TabId::new(9)),
        ],
    );

    assert_eq!(state.tab_project_labels[&TabId::new(9)], "hocus + pocus");
    assert!(
        !state.auto_tab_projects.contains_key(&TabId::new(9)),
        "two projects still assign nothing"
    );
}

#[test]
fn a_tab_assignment_does_not_override_its_panes_label() {
    let mut state = state_with_tab(TabId::new(9));
    state.run_local(&agent_call(
        "project_assign",
        r#"{"name":"named","root":"/work/named"}"#,
    ));
    poll(&mut state, vec![agent_in(3, "/work/hocus", TabId::new(9))]);

    assert_eq!(state.tab_project_labels[&TabId::new(9)], "hocus");
    assert_eq!(state.tab_project(TabId::new(9)), Some("named"));
}

#[test]
fn suppressed_snapshots_update_labels_without_adding_agents_to_the_list() {
    let mut state = state_with_tab(TabId::new(9));
    for (name, root) in [("A", "/work/a"), ("B", "/work/b")] {
        state
            .projects
            .insert(name.into(), projects::validate(name, root).unwrap());
    }
    let visible = shell_in(3, "/work/a", TabId::new(9));
    let mut hidden = agent_in(4, "/work/b", TabId::new(9));
    hidden.status = agents::AgentStatus::Exited;
    let cases = [
        (vec![visible.clone()], vec![hidden.clone()], Some("A + B")),
        (vec![], vec![hidden.clone()], Some("B")),
        (vec![hidden], vec![], Some("B")),
        (vec![shell_in(3, "/unknown", TabId::new(9))], vec![], None),
    ];
    for (seq, (panes, suppressed, label)) in cases.into_iter().enumerate() {
        let expected = panes.clone();
        let mut message = agent_snapshot_with(seq as u64 + 1, panes);
        let mut reply: PollReply =
            serde_json::from_str(message.payload.as_deref().unwrap()).unwrap();
        reply.suppressed_panes = suppressed;
        message.payload = Some(serde_json::to_string(&reply).unwrap());
        state.pipe(message);
        assert_eq!(state.panes, expected);
        assert_eq!(
            state
                .tab_project_labels
                .get(&TabId::new(9))
                .map(String::as_str),
            label
        );
    }
}

#[test]
fn a_tab_that_closed_loses_its_label() {
    let mut state = state_with_tab(TabId::new(9));
    poll(&mut state, vec![agent_in(3, "/work/hocus", TabId::new(9))]);
    assert_eq!(state.tab_project_labels[&TabId::new(9)], "hocus");

    state.on_tabs(vec![TabInfo {
        active: true,
        tab_id: TabId::new(10),
        ..TabInfo::default()
    }]);

    assert!(state.tab_project_labels.is_empty());
}

#[test]
fn a_shell_folder_becomes_no_project() {
    let mut state = state_with_tab(TabId::new(9));
    poll(&mut state, vec![shell_in(4, "/work/other", TabId::new(9))]);

    assert!(state.projects.is_empty());
}

/// The two widths side by side. A shell's checkout is enough to *name* the tab
/// it is in, and still not enough to remember the folder as a project — which is
/// what keeps every `cd` from filling the registry.
#[test]
fn a_shell_names_its_tab_without_becoming_a_project() {
    let mut state = state_with_tab(TabId::new(9));
    poll(&mut state, vec![shell_in(4, "/work/hocus", TabId::new(9))]);

    assert_eq!(state.tab_project_labels[&TabId::new(9)], "hocus");
    assert!(state.projects.is_empty(), "no folder was remembered");
    assert!(
        state.auto_tab_projects.is_empty(),
        "and no tab was assigned: only an agent's work assigns one",
    );
}

/// An unregistered checkout still names a tab, by its repository. Naming reverts
/// the moment the pane closes, so it can afford to answer where an assignment,
/// which outlives the session, cannot.
#[test]
fn an_unregistered_checkout_names_its_tab_after_the_repository() {
    let mut state = state_with_tab(TabId::new(9));
    poll(
        &mut state,
        vec![shell_in(4, "/somewhere/zellij", TabId::new(9))],
    );

    assert_eq!(state.tab_project_labels[&TabId::new(9)], "zellij");
    assert!(state.projects.is_empty());
}

/// A folder Git does not claim names nothing, or a shell wandering into a home
/// directory would rename the tab after it.
#[test]
fn a_plain_folder_names_no_tab() {
    let mut state = state_with_tab(TabId::new(9));
    poll_with(
        &mut state,
        vec![shell_in(4, "/home/me/Downloads", TabId::new(9))],
        folder,
    );

    assert!(state.tab_project_labels.is_empty());
}

#[test]
fn a_tab_whose_agents_share_a_project_is_assigned_automatically() {
    let mut state = state_with_tab(TabId::new(9));
    poll(
        &mut state,
        vec![
            agent_in(3, "/work/hocus", TabId::new(9)),
            agent_in(4, "/work/hocus", TabId::new(9)),
        ],
    );

    assert_eq!(state.auto_tab_projects[&TabId::new(9)], "hocus");
    assert!(
        state.tab_projects.is_empty(),
        "nothing was assigned by hand"
    );
    assert_eq!(state.active_project.as_deref(), Some("hocus"));
}

#[test]
fn a_tab_with_agents_from_two_projects_gets_no_assignment() {
    let mut state = state_with_tab(TabId::new(9));
    poll(
        &mut state,
        vec![
            agent_in(3, "/work/hocus", TabId::new(9)),
            agent_in(4, "/other/pocus", TabId::new(9)),
        ],
    );

    assert_eq!(state.projects.len(), 2);
    assert!(state.auto_tab_projects.is_empty());
    assert!(state.active_project.is_none());
}

/// An agent in a VM is working on the checkout the VM mirrors, so its tab takes that
/// project — the same answer an agent sitting in the worktree would give.
#[test]
fn a_tab_of_firecracker_agents_takes_the_project_their_vms_mirror() {
    let mut state = state_with_tab(TabId::new(9));
    poll(
        &mut state,
        vec![vm_agent_in(3, "/work/hocus", TabId::new(9))],
    );

    assert_eq!(state.auto_tab_projects[&TabId::new(9)], "hocus");
    assert_eq!(state.active_project.as_deref(), Some("hocus"));
}

/// And it counts as a vote, so a tab that mixes one with a local agent elsewhere is a
/// tab about two projects and gets no assignment. Before a VM agent had a project it
/// contributed nothing and the local one won alone.
#[test]
fn a_tab_mixing_a_local_agent_and_a_firecracker_agent_elsewhere_gets_no_assignment() {
    let mut state = state_with_tab(TabId::new(9));
    poll(
        &mut state,
        vec![
            agent_in(3, "/work/hocus", TabId::new(9)),
            vm_agent_in(4, "/other/pocus", TabId::new(9)),
        ],
    );

    assert_eq!(state.projects.len(), 2);
    assert!(state.auto_tab_projects.is_empty());
    assert!(state.active_project.is_none());
}

#[test]
fn an_explicit_assignment_wins_over_the_automatic_one() {
    let mut state = state_with_tab(TabId::new(9));
    state.run_local(&agent_call(
        "project_assign",
        r#"{"name":"named","root":"/work/named"}"#,
    ));
    poll(&mut state, vec![agent_in(3, "/work/hocus", TabId::new(9))]);

    assert_eq!(state.tab_project(TabId::new(9)), Some("named"));
    assert_eq!(state.active_project.as_deref(), Some("named"));
    assert!(
        state.auto_tab_projects.is_empty(),
        "an assigned tab is left alone",
    );
}

/// hocus may never learn which tab is active. The focused agent is the answer
/// either way.
#[test]
fn the_focused_agent_names_the_project_with_no_tabs_at_all() {
    let mut state = idle_state();
    poll_focused(
        &mut state,
        vec![agent_in(3, "/work/hocus", TabId::new(9))],
        Some(3),
    );

    assert!(state.tabs.is_empty(), "no TabUpdate ever arrived");
    assert_eq!(state.active_project.as_deref(), Some("hocus"));
}

#[test]
fn focusing_another_agent_moves_the_active_project() {
    let mut state = idle_state();
    let panes = vec![
        agent_in(3, "/work/hocus", TabId::new(9)),
        agent_in(4, "/other/pocus", TabId::new(9)),
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
        ..agent_in(4, "/work/hocus", TabId::new(9))
    };
    let panes = vec![agent_in(3, "/work/hocus", TabId::new(9)), shell];
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
    let mut state = state_with_tab(TabId::new(9));
    poll_focused(
        &mut state,
        vec![agent_in(3, "/work/hocus", TabId::new(9))],
        Some(3),
    );
    assert_eq!(state.last_focused_agent, Some(3));

    poll_focused(
        &mut state,
        vec![agent_in(4, "/other/pocus", TabId::new(9))],
        None,
    );
    assert_eq!(state.last_focused_agent, None);
    assert_eq!(state.active_project.as_deref(), Some("pocus"), "by tab");
}

/// Same precedence `tab_project` sets: what the user said by hand outranks
/// what was worked out from a checkout.
#[test]
fn an_explicit_assignment_wins_over_the_focused_agents_folder() {
    let mut state = state_with_tab(TabId::new(9));
    state.run_local(&agent_call(
        "project_assign",
        r#"{"name":"named","root":"/work/named"}"#,
    ));
    poll_focused(
        &mut state,
        vec![agent_in(3, "/work/hocus", TabId::new(9))],
        Some(3),
    );

    assert_eq!(state.active_project.as_deref(), Some("named"));
}

#[test]
fn a_closed_tab_drops_its_automatic_assignment() {
    let mut state = state_with_tab(TabId::new(9));
    poll(&mut state, vec![agent_in(3, "/work/hocus", TabId::new(9))]);
    assert_eq!(state.auto_tab_projects[&TabId::new(9)], "hocus");

    state.on_tabs(vec![TabInfo {
        active: true,
        tab_id: TabId::new(10),
        ..TabInfo::default()
    }]);
    assert!(state.auto_tab_projects.is_empty());
    assert!(state.active_project.is_none());
}

#[test]
fn naming_a_discovered_project_leaves_one_project_on_the_root() {
    let mut state = state_with_tab(TabId::new(9));
    poll(&mut state, vec![agent_in(3, "/work/hocus", TabId::new(9))]);
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
        suppressed_panes: Vec::new(),
        panes: Some(vec![agents::AgentPane {
            pane_id: 7,
            title: "claude".to_owned(),
            command: Some("claude --resume".to_owned()),
            status: agents::AgentStatus::Running,
            tab: Some((TabId::new(2), "work".to_owned())),
            cwd: Some("/tmp/work".to_owned()),
            host: None,
            firecracker_image: None,
            firecracker_worktree: None,
            detected: None,
            agent: None,
        }]),
        activity,
        manual_kinds: BTreeMap::new(),
        zellij_session_name: "test-session".to_owned().into(),
        tab_projects: BTreeMap::new(),
        repositories: BTreeMap::from([(
            Location::local("/tmp/work"),
            projects::Repository {
                root: "/tmp/work".to_owned(),
                remote: Some("git@example.com:work.git".to_owned()),
                worktree_status: Some(projects::WorktreeStatus::Linked),
                common_dir: Some("/tmp/main/.git".to_owned()),
                branch: Some("topic".to_owned()),
                host: None,
            },
        )]),
        focused: Some(7),
        summaries: agents::Summaries::new(),
        hooks: agents::HookFeeds::new(),
        runs: Vec::new(),
    };
    let json = serde_json::to_string(&reply).expect("serialize");
    assert!(serde_json::from_str::<serde_json::Value>(&json)
        .unwrap()
        .get("focused_plugin")
        .is_none());
    let back: PollReply = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.panes, reply.panes);
    assert_eq!(back.activity, reply.activity);
    assert_eq!(back.repositories, reply.repositories);
    assert_eq!(back.focused, reply.focused);
}
