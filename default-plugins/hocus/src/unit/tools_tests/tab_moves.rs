use super::*;

#[test]
fn a_tab_move_reports_the_stable_id_the_panes_landed_in() {
    // Zellij answers with the tab's stable id, which is the one thing every
    // tab tool takes, so it is handed straight back to the model.
    assert_eq!(moved_panes_result(Some(47)), "moved to tab_id 47");
    assert!(
        moved_panes_result(None).starts_with("zellij did not move the panes"),
        "and a failure says so rather than claiming a move"
    );
}

/// A global pane is in the session, so the membership check would pass it straight
/// through to a host that refuses it silently. It is answered first, and by name.
#[test]
fn moving_a_global_pane_to_a_tab_says_it_is_global_rather_than_missing() {
    let error = check_move_to_tab(
        &MoveDestination::ExistingId(2),
        &[PaneId::Terminal(7)],
        &diverged_session(),
    )
    .unwrap_err();
    assert!(error.contains("global pane on the bottom edge"), "{error}");
    assert!(
        error.contains("move_panes_to_tab cannot act on it"),
        "{error}"
    );
    assert!(error.contains("set_global_pane_placement"), "{error}");
    assert!(!error.contains("is not in this session"), "{error}");

    // Even for a new tab, which takes no tab_id and so passes every other check.
    let error = check_move_to_tab(
        &MoveDestination::New(None),
        &[PaneId::Terminal(7)],
        &diverged_session(),
    )
    .unwrap_err();
    assert!(error.contains("global pane"), "{error}");
}

#[test]
fn moving_panes_to_a_missing_tab_id_names_the_id() {
    let error = check_move_to_tab(
        &MoveDestination::ExistingId(9),
        &[PaneId::Terminal(2)],
        &diverged_session(),
    )
    .unwrap_err();
    assert!(error.contains("tab_id 9"), "{error}");
}

/// The whole point of addressing tabs by id alone: a display position that
/// happens to name a real tab in the tab bar is still not a `tab_id`, and is
/// refused rather than silently acting on whichever tab owns that id.
#[test]
fn a_display_position_is_refused_where_a_tab_id_is_required() {
    let session = diverged_session();
    let panes = [PaneId::Terminal(2)];
    // ids 2 and 3 are the session's two tabs
    assert!(check_move_to_tab(&MoveDestination::ExistingId(2), &panes, &session).is_ok());
    assert!(check_move_to_tab(&MoveDestination::ExistingId(3), &panes, &session).is_ok());
    // ...while positions 0 and 1 name the very same two tabs and are not ids
    assert!(check_move_to_tab(&MoveDestination::ExistingId(0), &panes, &session).is_err());
    let error = check_move_to_tab(&MoveDestination::ExistingId(1), &panes, &session).unwrap_err();
    assert!(error.contains("tab_id 1"), "{error}");
    assert!(error.contains("not a position"), "{error}");
}

#[test]
fn moving_a_pane_that_is_not_in_the_session_names_the_pane() {
    let error = check_move_to_tab(
        &MoveDestination::ExistingId(3),
        &[PaneId::Terminal(2), PaneId::Terminal(99)],
        &diverged_session(),
    )
    .unwrap_err();
    assert!(error.contains("pane 99"), "{error}");
}

#[test]
fn moving_panes_to_a_new_tab_has_no_destination_to_check() {
    assert!(check_move_to_tab(
        &MoveDestination::New(None),
        &[PaneId::Terminal(2)],
        &SessionTabs::default()
    )
    .is_err());
    assert!(check_move_to_tab(
        &MoveDestination::New(None),
        &[PaneId::Terminal(2)],
        &diverged_session()
    )
    .is_ok());
}
