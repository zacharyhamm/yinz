use super::*;

fn tabs(ids: &[usize]) -> Vec<TabInfo> {
    ids.iter()
        .map(|id| TabInfo {
            tab_id: TabId::new(*id),
            ..Default::default()
        })
        .collect()
}

#[test]
fn closed_tab_assignments_stay_deleted_after_reload_without_touching_unobserved_tabs() {
    let backend = store::Local::shared("daemon");
    backend.define_all(&store::shared_defs()).unwrap();
    let project_id = backend
        .apply(vec![projects::to_mutation(
            &projects::validate("work", "/project").unwrap(),
        )])
        .unwrap()
        .ids[0];
    let mut state = State {
        host_id: HostId::new("local"),
        yinz_session_name: Some(SessionName::new("session")),
        ..Default::default()
    };
    let assignment = |host: &str, session: &str, tab: i64| {
        Mutation::put("agent_assignment")
            .set("host_id", host)
            .set("yinz_session_name", session)
            .set("tab_id", tab)
            .set("project", "work")
            .set("project_id", project_id.get())
            .build()
    };
    backend
        .apply(vec![
            assignment("local", "session", 1),
            assignment("local", "session", 2),
            assignment("local", "session", 3),
            assignment("other", "session", 1),
            assignment("local", "other", 1),
        ])
        .unwrap();
    state.restore_assignments_from(&backend).unwrap();
    state
        .prune_closed_tab_assignments(&tabs(&[1, 2]), &backend)
        .unwrap();
    assert_eq!(
        state.tab_projects.len(),
        3,
        "initial discovery preserves restored assignments"
    );
    state.tabs = tabs(&[1, 2]);
    // project_open persists before the new tab's first event reaches the daemon.
    backend
        .apply(vec![assignment("local", "session", 9)])
        .unwrap();
    state.tab_projects.insert(TabId::new(9), "work".into());
    state
        .prune_closed_tab_assignments(&tabs(&[2]), &backend)
        .unwrap();
    assert_eq!(
        state.tab_projects.keys().copied().collect::<Vec<_>>(),
        vec![TabId::new(2), TabId::new(3), TabId::new(9)]
    );
    state.tab_projects.clear();
    state.restore_assignments_from(&backend).unwrap();
    assert_eq!(
        state.tab_projects.keys().copied().collect::<Vec<_>>(),
        vec![TabId::new(2), TabId::new(3), TabId::new(9)]
    );
    assert_eq!(
        backend
            .count(&FetchRequest::entity("agent_assignment"))
            .unwrap(),
        5
    );
    state.tabs = tabs(&[2, 3, 9]);
    state.prune_closed_tab_assignments(&[], &backend).unwrap();
    state.restore_assignments_from(&backend).unwrap();
    assert!(state.tab_projects.is_empty());
    assert_eq!(
        backend
            .count(&FetchRequest::entity("agent_assignment"))
            .unwrap(),
        2
    );
}

#[test]
fn failed_closed_tab_persistence_is_reported_and_keeps_the_cached_assignment() {
    let backend = store::Local::shared("undefined-entities");
    let mut state = State {
        tabs: tabs(&[1]),
        ..Default::default()
    };
    state.tab_projects.insert(TabId::new(1), "work".into());
    assert!(state.prune_closed_tab_assignments(&[], &backend).is_err());
    assert!(state.tab_projects.contains_key(&TabId::new(1)));
}

#[test]
fn reload_scopes_manual_metadata_by_host_and_follows_project_identity() {
    let backend = store::Local::shared("daemon");
    backend.define_all(&store::shared_defs()).unwrap();
    let project = projects::validate("first", "/project").unwrap();
    let report = backend
        .apply(vec![projects::to_mutation(&project)])
        .unwrap();
    let mut state = State {
        host_id: HostId::new("local"),
        yinz_session_name: Some(SessionName::new("same-name")),
        ..Default::default()
    };
    backend
        .apply(vec![
            agents::rows::latch_mutation(&state.owner(), 7, "reviewer"),
            agents::rows::latch_mutation(
                &agents::rows::Owner::new(HostId::new("elsewhere"), SessionName::new("same-name")),
                7,
                "other",
            ),
            Mutation::put("agent_assignment")
                .set("host_id", "local")
                .set("yinz_session_name", "same-name")
                .set("tab_id", 3_i64)
                .set("project", "first")
                .set("project_id", report.ids[0].get())
                .build(),
        ])
        .unwrap();
    state.restore_assignments_from(&backend).unwrap();
    assert_eq!(
        state.manual_kinds.get(&7).map(String::as_str),
        Some("reviewer")
    );
    assert_eq!(
        state.tab_projects.get(&TabId::new(3)).map(String::as_str),
        Some("first")
    );
    let rows = backend.fetch(&FetchRequest::entity("project")).unwrap();
    backend
        .apply(
            projects::plan_rename(&rows, "first", "renamed")
                .unwrap()
                .mutations,
        )
        .unwrap();
    state.restore_assignments_from(&backend).unwrap();
    assert_eq!(
        state.tab_projects.get(&TabId::new(3)).map(String::as_str),
        Some("renamed")
    );
    backend
        .apply(vec![
            projects::delete_mutation("renamed"),
            projects::to_mutation(&project),
        ])
        .unwrap();
    state.restore_assignments_from(&backend).unwrap();
    assert!(
        state.tab_projects.is_empty(),
        "recreated names do not claim old assignments"
    );
}
