use super::*;
use zellij_types::internal_ids::TabId;

#[test]
fn break_panes_to_tab_with_id_moves_a_pane_when_id_and_position_diverge() {
    let mut screen = screen_with_diverged_tab_ids();
    let pane_to_move = PaneId::Terminal(crate::TerminalId::new(42));
    add_pane_to_tab(&mut screen, TabId::new(2), pane_to_move, false);

    // tab id 3 is at position 1, so a function that read this argument as a position
    // would find no tab at all and quietly do nothing
    let outcome = screen
        .break_multiple_panes_to_tab_with_id(
            vec![pane_to_move],
            TabId::new(3),
            false,
            crate::ClientId::new(1),
        )
        .expect("TEST");

    assert!(outcome.succeeded(), "{outcome:?}");
    assert_eq!(outcome.moved, vec![pane_to_move]);
    assert_eq!(outcome.destination_tab_id, Some(TabId::new(3)));
    assert!(
        screen
            .tabs
            .get(&TabId::new(3))
            .unwrap()
            .has_pane_with_pid(&pane_to_move),
        "The pane is in the tab it was addressed to"
    );
    assert!(
        !screen
            .tabs
            .get(&TabId::new(2))
            .unwrap()
            .has_pane_with_pid(&pane_to_move),
        "and no longer in the tab it came from"
    );
}

#[test]
fn break_panes_to_tab_with_id_is_a_no_op_for_a_nonexistent_tab_id() {
    let mut screen = screen_with_diverged_tab_ids();
    let pane_to_move = PaneId::Terminal(crate::TerminalId::new(42));
    add_pane_to_tab(&mut screen, TabId::new(2), pane_to_move, false);

    let outcome = screen
        .break_multiple_panes_to_tab_with_id(
            vec![pane_to_move],
            TabId::new(99),
            false,
            crate::ClientId::new(1),
        )
        .expect("TEST");

    assert!(!outcome.succeeded(), "{outcome:?}");
    assert_eq!(
        outcome.destination_tab_id, None,
        "No destination means nothing was even extracted"
    );
    assert_eq!(outcome.affected_tab_id(), None, "so the plugin is told no");
    assert!(
        screen
            .tabs
            .get(&TabId::new(2))
            .unwrap()
            .has_pane_with_pid(&pane_to_move),
        "The pane is untouched"
    );
    assert_eq!(screen.tabs.len(), 2, "and so are the tabs");
}

#[test]
fn break_panes_to_tab_never_destroys_a_pane_it_could_not_move() {
    let mut screen = screen_with_diverged_tab_ids();
    let pane_to_move = PaneId::Terminal(crate::TerminalId::new(42));
    add_pane_to_tab(&mut screen, TabId::new(2), pane_to_move, false);

    // every way of naming a tab that does not exist, including the ids of the two tabs
    // that were closed and the positions the surviving tabs no longer have
    for bad_tab_id in [0, 1, 4, 99] {
        let outcome = screen
            .break_multiple_panes_to_tab_with_id(
                vec![pane_to_move],
                TabId::new(bad_tab_id),
                false,
                crate::ClientId::new(1),
            )
            .expect("TEST");
        assert!(!outcome.succeeded(), "tab id {bad_tab_id}: {outcome:?}");
        assert!(
            screen
                .tabs
                .get(&TabId::new(2))
                .unwrap()
                .has_pane_with_pid(&pane_to_move),
            "tab id {bad_tab_id}: the pane is still in the tab it came from"
        );
        assert!(
            outcome.closed.is_empty(),
            "tab id {bad_tab_id}: and nothing was torn down"
        );
    }
}

#[test]
fn break_panes_to_tab_leaves_a_pane_that_is_already_in_the_destination() {
    let mut screen = screen_with_diverged_tab_ids();
    let pane = PaneId::Terminal(crate::TerminalId::new(42));
    add_pane_to_tab(&mut screen, TabId::new(3), pane, false);

    let outcome = screen
        .break_multiple_panes_to_tab_with_id(
            vec![pane],
            TabId::new(3),
            false,
            crate::ClientId::new(1),
        )
        .expect("TEST");

    assert!(outcome.succeeded(), "{outcome:?}");
    assert_eq!(outcome.already_in_destination, vec![pane]);
    assert!(outcome.moved.is_empty(), "nothing needed moving");
    assert!(screen
        .tabs
        .get(&TabId::new(3))
        .unwrap()
        .has_pane_with_pid(&pane));
}

#[test]
fn break_panes_to_tab_with_id_keeps_a_floating_pane_floating() {
    let mut screen = screen_with_diverged_tab_ids();
    let pane_to_move = PaneId::Terminal(crate::TerminalId::new(42));
    add_pane_to_tab(&mut screen, TabId::new(2), pane_to_move, true);
    assert!(
        screen
            .tabs
            .get(&TabId::new(2))
            .unwrap()
            .pane_id_is_floating(&pane_to_move),
        "Pre-condition: the pane starts floating"
    );

    let outcome = screen
        .break_multiple_panes_to_tab_with_id(
            vec![pane_to_move],
            TabId::new(3),
            false,
            crate::ClientId::new(1),
        )
        .expect("TEST");

    assert!(outcome.succeeded(), "{outcome:?}");
    assert!(
        screen
            .tabs
            .get(&TabId::new(3))
            .unwrap()
            .pane_id_is_floating(&pane_to_move),
        "and arrives floating rather than tiled"
    );
}

#[test]
fn break_panes_to_tab_with_id_focuses_the_destination_tab() {
    let mut screen = screen_with_diverged_tab_ids();
    let pane_to_move = PaneId::Terminal(crate::TerminalId::new(42));
    add_pane_to_tab(&mut screen, TabId::new(2), pane_to_move, false);
    screen
        .switch_active_tab(0, None, true, crate::ClientId::new(1))
        .expect("TEST");

    screen
        .break_multiple_panes_to_tab_with_id(
            vec![pane_to_move],
            TabId::new(3),
            true,
            crate::ClientId::new(1),
        )
        .expect("TEST");

    assert_eq!(
        screen.active_tab_ids.get(&ClientId::new(1)),
        Some(&TabId::new(3)),
        "Focus follows the panes, and lands on the tab id rather than the position"
    );
}

#[test]
fn break_panes_to_tab_reports_failure_when_only_some_panes_moved() {
    let mut screen = screen_with_diverged_tab_ids();
    let real_pane = PaneId::Terminal(crate::TerminalId::new(42));
    let stale_pane = PaneId::Terminal(crate::TerminalId::new(43));
    add_pane_to_tab(&mut screen, TabId::new(2), real_pane, false);

    let outcome = screen
        .break_multiple_panes_to_tab_with_id(
            vec![real_pane, stale_pane],
            TabId::new(3),
            false,
            crate::ClientId::new(1),
        )
        .expect("TEST");

    assert_eq!(outcome.moved, vec![real_pane], "the real pane did move");
    assert_eq!(outcome.not_found, vec![stale_pane]);
    assert!(
        !outcome.succeeded(),
        "but a partial move is reported as a failure, so the caller re-reads the session"
    );
    assert_eq!(outcome.affected_tab_id(), None);
}

/// The `BreakPanesToTabWithIndex` handler takes a 0-based display position, which it
/// must convert to a stable id before moving anything. This is the exact call that
/// destroyed a pane in the field: position 1 addressed a tab whose id was 3, and the
/// mover then looked up tab *id* 1, found nothing, and dropped the pane it had already
/// pulled out of its tab.
#[test]
fn break_panes_to_tab_with_index_resolves_a_position_and_not_an_id() {
    let mut screen = screen_with_diverged_tab_ids();
    let pane_to_move = PaneId::Terminal(crate::TerminalId::new(42));
    add_pane_to_tab(&mut screen, TabId::new(2), pane_to_move, false);

    let tab_id = screen
        .get_tab_id_at_position(1)
        .expect("TEST: position 1 exists");
    assert_eq!(
        tab_id,
        TabId::new(3),
        "position 1 is tab id 3, not tab id 1"
    );
    let outcome = screen
        .break_multiple_panes_to_tab_with_id(
            vec![pane_to_move],
            tab_id,
            false,
            crate::ClientId::new(1),
        )
        .expect("TEST");

    assert!(outcome.succeeded(), "{outcome:?}");
    assert!(
        screen
            .tabs
            .get(&TabId::new(3))
            .unwrap()
            .has_pane_with_pid(&pane_to_move),
        "The pane is in the tab the user pointed at"
    );
    assert_eq!(screen.tabs.len(), 2, "and no tab was swept away");
}

#[test]
fn break_panes_to_tab_with_index_has_no_tab_to_resolve_when_the_position_is_out_of_range() {
    let screen = screen_with_diverged_tab_ids();
    assert_eq!(
        screen.get_tab_id_at_position(7),
        None,
        "so the handler refuses before extracting anything"
    );
}
