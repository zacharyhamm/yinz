use super::*;

/// A change to rows this plugin is not watching is somebody else's.
#[test]
fn a_change_for_another_watch_is_ignored() {
    let mut state = state_with_project();
    let rows = projects::to_rows(&projects::Registry::new());

    assert!(!state.on_projects_changed(state.project_watch + 1, &rows));
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
        zellij_session_name: "test-session".to_owned(),
        ..State::default()
    };
    restarted
        .store()
        .apply(vec![agents::rows::summary_mutation(
            "test-session",
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
