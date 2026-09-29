use super::*;
use yinz_tile::prelude::DataWatchId;
use yinz_tile::prelude::HostId;

fn state_with_duplicate_projects() -> State {
    let mut state = state_with_project();
    let source = state.projects.get_mut("hocus").unwrap();
    source.remote = Some("git@example.com:owner/repo.git".into());
    let mut target = projects::validate("kept", "/kept").unwrap();
    target.remote = Some("https://example.com/owner/repo".into());
    target.discovered = true;
    state.projects.insert("kept".into(), target);
    state
        .store()
        .apply(state.projects.values().map(projects::to_mutation).collect())
        .unwrap();
    state
}

#[test]
fn project_slash_commands_persist_then_retarget_assignments() {
    let mut state = state_with_duplicate_projects();
    settle(&mut state);
    add_background_store(&mut state);
    state.granted = true;
    state.sync_project_skills();
    load_skill(
        &mut state,
        skills::Source::Project,
        "source-skill",
        "Source.",
    );
    state
        .input
        .set_text("/project_rename name=hocus new_name=renamed".into());
    state.submit();
    finish_background_project_change(&mut state);
    assert_eq!(state.active_project.as_deref(), Some("renamed"));
    assert_eq!(state.tab_projects[&TabId::new(9)], "renamed");
    assert!(state.skills.get("source-skill").is_some());
    assert!(tool_results(&state)
        .last()
        .unwrap()
        .content
        .contains("project_rename"));
    state
        .input
        .set_text("/project_merge source=renamed target=kept".into());
    state.submit();
    finish_background_project_change(&mut state);
    assert_eq!(state.active_project.as_deref(), Some("kept"));
    assert_eq!(state.tab_projects[&TabId::new(9)], "kept");
    assert_eq!(state.skills_project_root.as_deref(), Some("/kept"));
    assert!(state.skills.get("source-skill").is_none());
    assert_eq!(state.projects.len(), 1);
    assert!(!state.projects["kept"].discovered);
    assert_eq!(state.sent_tab_projects, state.tab_projects);
    assert!(
        state.tab_project_labels.is_empty(),
        "assignments alone do not name tabs"
    );
    let rows = state
        .store()
        .fetch(&FetchRequest::entity("project"))
        .unwrap();
    assert_eq!(projects::from_rows(&rows), state.projects);
    assert!(tool_results(&state)
        .last()
        .unwrap()
        .content
        .contains("project_merge"));
}

#[test]
fn a_project_write_conflict_leaves_memory_and_assignments_unchanged() {
    for merge in [false, true] {
        let mut state = state_with_duplicate_projects();
        let rows = state
            .store()
            .fetch(&FetchRequest::entity("project"))
            .unwrap();
        let change = if merge {
            projects::plan_merge(&rows, "hocus", "kept").unwrap()
        } else {
            projects::plan_rename(&rows, "hocus", "renamed").unwrap()
        };
        state
            .store()
            .apply(vec![Mutation::update("project", Pred::eq("name", "hocus"))
                .set("worktree_dir", "concurrent")
                .build()])
            .unwrap();
        let before = state.projects.clone();
        let assigned = state.tab_projects.clone();
        let auto = state.auto_tab_projects.clone();
        let labels = state.tab_project_labels.clone();
        let sent = state.sent_tab_projects.clone();
        let active = state.active_project.clone();
        assert!(state
            .apply_project_change(&rows, change)
            .unwrap_err()
            .contains("projects changed"));
        assert_eq!(state.projects, before);
        assert_eq!(state.tab_projects, assigned);
        assert_eq!(state.auto_tab_projects, auto);
        assert_eq!(state.tab_project_labels, labels);
        assert_eq!(state.sent_tab_projects, sent);
        assert_eq!(state.active_project, active);
    }
}

#[test]
fn project_watch_requires_explicit_replacements_even_with_shared_roots_or_origins() {
    for operation in ["forget", "rename", "merge", "adopt", "successive", "reuse"] {
        for shared_root in [false, true] {
            let mut state = state_with_duplicate_projects();
            assert!(state.panes.is_empty(), "the tab is assigned manually");
            let backend = state.store();
            let mut other = state.projects["hocus"].clone();
            other.name = "other".into();
            if !shared_root {
                other.root = "/other".into();
            }
            // Merging with a shared source root is allowed only if the target keeps it.
            let mut kept = state.projects["kept"].clone();
            if shared_root {
                kept.root = other.root.clone();
            }
            let mut source = state.projects["hocus"].clone();
            source.discovered = operation == "adopt";
            backend
                .apply(vec![
                    projects::to_mutation(&other),
                    projects::to_mutation(&kept),
                    projects::to_mutation(&source),
                ])
                .unwrap();
            let rows = backend.fetch(&FetchRequest::entity("project")).unwrap();
            drop(backend);
            state.on_projects_changed(state.project_watch, &rows);
            let backend = state.store();
            let mutations = match operation {
                "rename" | "successive" => {
                    projects::plan_rename(&rows, "hocus", "renamed")
                        .unwrap()
                        .mutations
                },
                "merge" => {
                    projects::plan_merge(&rows, "hocus", "kept")
                        .unwrap()
                        .mutations
                },
                "adopt" => {
                    projects::plan_adopt(
                        &rows,
                        projects::validate("renamed", "/work/hocus").unwrap(),
                        false,
                    )
                    .unwrap()
                    .1
                },
                _ => vec![projects::delete_mutation("hocus")],
            };
            backend.apply(mutations).unwrap();
            if operation == "successive" {
                let rows = backend.fetch(&FetchRequest::entity("project")).unwrap();
                backend
                    .apply(
                        projects::plan_merge(&rows, "renamed", "kept")
                            .unwrap()
                            .mutations,
                    )
                    .unwrap();
                let rows = backend.fetch(&FetchRequest::entity("project")).unwrap();
                backend
                    .apply(
                        projects::plan_rename(&rows, "kept", "last")
                            .unwrap()
                            .mutations,
                    )
                    .unwrap();
            }
            if operation == "reuse" {
                backend.apply(vec![projects::to_mutation(&source)]).unwrap();
            }
            let rows = backend.fetch(&FetchRequest::entity("project")).unwrap();
            drop(backend);
            state.on_projects_changed(state.project_watch, &rows);
            let expected = match operation {
                "rename" | "adopt" => Some("renamed"),
                "merge" => Some("kept"),
                "successive" => Some("last"),
                _ => None,
            };
            assert_eq!(
                state.tab_projects.get(&TabId::new(9)).map(String::as_str),
                expected,
                "{operation} shared={shared_root}"
            );
            assert_eq!(state.active_project.as_deref(), expected);
        }
    }
}

#[test]
fn a_reused_name_does_not_follow_the_old_rows_replacement() {
    let mut state = state_with_duplicate_projects();
    state.run_local(&agent_call(
        "project_rename",
        r#"{"name":"hocus","new_name":"renamed"}"#,
    ));
    state.run_local(&agent_call(
        "project_assign",
        r#"{"name":"hocus","root":"/work/hocus"}"#,
    ));
    let backend = state.store();
    let rows = backend.fetch(&FetchRequest::entity("project")).unwrap();
    assert!(projects::from_rows(&rows)["hocus"].replaced_ids.is_empty());
    backend
        .apply(vec![projects::delete_mutation("hocus")])
        .unwrap();
    let rows = backend.fetch(&FetchRequest::entity("project")).unwrap();
    drop(backend);
    state.on_projects_changed(state.project_watch, &rows);
    assert!(state.tab_projects.is_empty());
    assert!(state.active_project.is_none());
}

#[test]
fn adoption_publishes_row_history_and_settings_updates_preserve_it() {
    let mut state = state_with_project();
    let mut found = state.projects["hocus"].clone();
    found.discovered = true;
    give_projects(
        &mut state,
        projects::Registry::from([("hocus".into(), found)]),
    );
    let old_id = state.project_ids["hocus"];
    state.run_local(&agent_call(
        "project_assign",
        r#"{"name":"mine","root":"/work/hocus"}"#,
    ));
    assert_eq!(state.projects["mine"].replaced_ids, vec![old_id]);
    for args in [
        r#"{"name":"mine","root":"/work/hocus"}"#,
        r#"{"name":"mine","worktree_dir":"/trees"}"#,
    ] {
        state.run_local(&agent_call("project_assign", args));
        assert_eq!(state.projects["mine"].replaced_ids, vec![old_id]);
        let rows = state
            .store()
            .fetch(&FetchRequest::entity("project"))
            .unwrap();
        assert_eq!(projects::from_rows(&rows), state.projects);
    }
}

/// A change to rows this plugin is not watching is somebody else's.
#[test]
fn a_change_for_another_watch_is_ignored() {
    let mut state = state_with_project();
    let rows = projects::to_rows(&projects::Registry::new());

    assert!(!state.on_projects_changed(DataWatchId::new(state.project_watch.get() + 1), &rows));
    assert_eq!(
        state.active_project.as_deref(),
        Some("hocus"),
        "another plugin's watch must not clear ours"
    );
}

/// The watch is the only way projects arrive now, so a project that goes
/// away in one has to stop being the active project.
#[test]
fn a_change_that_drops_a_project_clears_what_named_it() {
    let mut state = state_with_project();
    assert_eq!(state.active_project.as_deref(), Some("hocus"));

    give_projects(&mut state, projects::Registry::new());
    assert_eq!(state.active_project, None);
    assert!(state.tab_projects.is_empty());
}

/// Opening the store takes the rows that are already there, so hocus has its
/// registry in hand rather than after a round trip.
#[test]
fn opening_the_store_takes_the_projects_already_in_it() {
    let mut state = State::default();
    let hocus = projects::validate("hocus", "/work/hocus").unwrap();
    state
        .store()
        .apply(vec![projects::to_mutation(&hocus)])
        .expect("written");

    state.open_store();
    assert_eq!(state.projects["hocus"].root, "/work/hocus");
}

/// Anything that is not a snapshot is somebody else's message — and every
/// plugin sees them, because the subscribe goes out as a broadcast.
#[test]
fn an_unknown_pipe_message_is_ignored() {
    let mut state = state_with_panes();
    assert!(!state.pipe(PipeMessage {
        source: PipeSource::Cli("1".to_owned()),
        name: "something_else".to_owned(),
        payload: Some("{}".to_owned()),
        args: BTreeMap::new(),
        is_private: false,
        pane_id: None,
    }));
    assert!(!state.panes.is_empty());
    assert_eq!(state.agent_seq, 0, "an unrelated message merges nothing");
}

/// The daemon's summaries are mirrored as they arrive - only the changed ones -
/// and are what a restarted hocus shows until the daemon speaks again.
#[test]
fn summaries_are_mirrored_and_restored_without_their_hash() {
    let mut state = state_with_panes();
    let summary = |text: &str, hash: u64| agents::Summary {
        text: text.to_owned(),
        title: "task".to_owned(),
        awaiting: false,
        hash,
        created_at_ms: None,
    };
    let stored = |state: &State| {
        let rows = state
            .store()
            .fetch(&FetchRequest::entity(agents::rows::SUMMARY_ENTITY))
            .expect("fetched");
        agents::rows::summaries_from_rows(&rows)
    };
    let panes = state.panes.clone();
    state.apply_poll(
        panes.clone(),
        BTreeMap::new(),
        BTreeMap::from([(3, summary("reviewing", 7))]),
        BTreeMap::new(),
        None,
    );
    assert_eq!(
        stored(&state),
        BTreeMap::from([(3, summary("reviewing", 0))])
    );

    // A poll that changes nothing but the hash writes nothing.
    let before = state
        .store()
        .count(&FetchRequest::entity(agents::rows::SUMMARY_ENTITY));
    state.apply_poll(
        panes.clone(),
        BTreeMap::new(),
        BTreeMap::from([(3, summary("reviewing", 8))]),
        BTreeMap::new(),
        None,
    );
    assert_eq!(
        state
            .store()
            .count(&FetchRequest::entity(agents::rows::SUMMARY_ENTITY)),
        before
    );

    // Named, because a summary belongs to a session as well as a pane - see
    // `store::agent_summary_def`.
    let mut restarted = State {
        yinz_session_name: "test-session".to_owned().into(),
        host_id: HostId::new("test-host"),
        ..State::default()
    };
    restarted
        .store()
        .apply(vec![agents::rows::summary_mutation(
            &restarted.owner(),
            3,
            &summary("reviewing", 7),
        )])
        .expect("copied");
    restarted.open_store();
    assert_eq!(
        restarted.summaries,
        BTreeMap::from([(3, summary("reviewing", 0))])
    );

    // A pane the daemon no longer summarizes is forgotten.
    state.apply_poll(
        panes,
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        None,
    );
    assert!(stored(&state).is_empty());
}
