//! `Screen::reveal_pane_with_id` - showing a client a pane without taking its
//! keyboard focus.
//!
//! The whole feature turns on one thing: `active_tab_ids` is at once the tab a
//! client sees and the tab it types into, so a tab switch normally relocates
//! focus. A global pane is the exception, and these pin that a reveal trades on
//! it rather than on luck.

use super::*;

/// Two tabs, a terminal in each, and a global pane down the left. `client_id` 1
/// starts on tab 0.
fn screen_with_two_tabs_and_a_global_pane() -> Screen {
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    insert_left_global_pane(&mut screen, 10);
    // `new_tab` leaves the client on the tab it just made; the reveal is only
    // interesting from somewhere else.
    screen.go_to_tab(1, 1).expect("TEST");
    screen
}

/// The case the whole thing exists for: keyboard focus is parked on the sidebar,
/// so the tab underneath it can be changed out without the user losing the keys.
#[test]
fn a_client_on_a_global_pane_is_carried_to_the_panes_tab_and_keeps_the_keyboard() {
    let client_id = 1;
    let mut screen = screen_with_two_tabs_and_a_global_pane();
    let global_pane_id = PaneId::Terminal(99);
    screen
        .focus_global_pane(global_pane_id, client_id)
        .expect("TEST");
    assert_eq!(screen.get_active_tab(client_id).unwrap().position, 0);

    screen
        .reveal_pane_with_id(
            PaneId::Terminal(2),
            Some("hocus: send_input?".into()),
            client_id,
        )
        .expect("TEST");

    assert_eq!(
        screen.get_active_tab(client_id).unwrap().position,
        1,
        "the tab holding the pane should have been brought into view"
    );
    assert_eq!(
        screen.get_active_pane_id(&client_id),
        Some(global_pane_id),
        "and the keyboard should not have gone with it"
    );
}

/// The client is typing into a pane in a tab. There is no way to change what it
/// sees without changing what it types into, so the view is left alone - losing
/// your place mid-keystroke is worse than not being shown the pane.
#[test]
fn a_client_focused_inside_a_tab_is_not_dragged_out_of_it() {
    let client_id = 1;
    let mut screen = screen_with_two_tabs_and_a_global_pane();
    let focused_before = screen.get_active_pane_id(&client_id);
    assert_eq!(focused_before, Some(PaneId::Terminal(1)));

    screen
        .reveal_pane_with_id(
            PaneId::Terminal(2),
            Some("hocus: send_input?".into()),
            client_id,
        )
        .expect("TEST");

    assert_eq!(
        screen.get_active_tab(client_id).unwrap().position,
        0,
        "the view should have stayed where the client was"
    );
    assert_eq!(screen.get_active_pane_id(&client_id), focused_before);
}

/// Even when the pane cannot be brought over, it gets marked - so the label is
/// waiting on it whenever the client does arrive.
#[test]
fn a_pane_that_cannot_be_revealed_is_still_marked() {
    let client_id = 1;
    let mut screen = screen_with_two_tabs_and_a_global_pane();

    screen
        .reveal_pane_with_id(
            PaneId::Terminal(2),
            Some("hocus: send_input?".into()),
            client_id,
        )
        .expect("TEST");

    let marked = screen
        .with_pane(PaneId::Terminal(2), |pane| {
            pane.frame_color_override(client_id)
        })
        .expect("the pane exists");
    assert!(marked.is_some(), "the pane should be wearing the mark");
}

/// The question belongs to the conversation the asking client is having. Putting
/// it on a second client's screen would be telling them about something they
/// never asked for.
#[test]
fn the_mark_is_drawn_only_for_the_client_it_was_raised_for() {
    let asking_client = 1;
    let other_client = 2;
    let mut screen = screen_with_two_tabs_and_a_global_pane();

    screen
        .reveal_pane_with_id(
            PaneId::Terminal(2),
            Some("hocus: send_input?".into()),
            asking_client,
        )
        .expect("TEST");

    let (asking, other) = screen
        .with_pane(PaneId::Terminal(2), |pane| {
            (
                pane.frame_color_override(asking_client),
                pane.frame_color_override(other_client),
            )
        })
        .expect("the pane exists");
    assert!(asking.is_some(), "the client that asked sees the mark");
    assert_eq!(other, None, "nobody else does");
}

/// And the clear is symmetric: a client only takes down a mark that was put up
/// for it, so one conversation cannot cancel another's question.
#[test]
fn a_client_cannot_clear_a_mark_raised_for_someone_else() {
    let asking_client = 1;
    let other_client = 2;
    let mut screen = screen_with_two_tabs_and_a_global_pane();
    screen
        .reveal_pane_with_id(PaneId::Terminal(2), None, asking_client)
        .expect("TEST");

    screen.with_pane(PaneId::Terminal(2), |pane| {
        pane.clear_pane_frame_color_override(Some(other_client))
    });
    let still_marked = screen
        .with_pane(PaneId::Terminal(2), |pane| {
            pane.frame_color_override(asking_client)
        })
        .expect("the pane exists");
    assert!(
        still_marked.is_some(),
        "the other client's clear is not theirs to give"
    );

    screen.with_pane(PaneId::Terminal(2), |pane| {
        pane.clear_pane_frame_color_override(Some(asking_client))
    });
    let cleared = screen
        .with_pane(PaneId::Terminal(2), |pane| {
            pane.frame_color_override(asking_client)
        })
        .expect("the pane exists");
    assert_eq!(cleared, None, "its own clear does");
}

/// A bell flash and a failed command belong to everyone, and their clears know
/// nothing about clients - so a clientless override is seen by, and cleared by,
/// all of them.
#[test]
fn an_override_raised_for_nobody_in_particular_belongs_to_every_client() {
    let mut screen = screen_with_two_tabs_and_a_global_pane();
    screen.with_pane(PaneId::Terminal(2), |pane| {
        pane.add_red_pane_frame_color_override(Some("exited 1".to_owned()))
    });

    let (first, second) = screen
        .with_pane(PaneId::Terminal(2), |pane| {
            (pane.frame_color_override(1), pane.frame_color_override(2))
        })
        .expect("the pane exists");
    assert!(first.is_some() && second.is_some());

    screen.with_pane(PaneId::Terminal(2), |pane| {
        pane.clear_pane_frame_color_override(None)
    });
    let cleared = screen
        .with_pane(PaneId::Terminal(2), |pane| pane.frame_color_override(1))
        .expect("the pane exists");
    assert_eq!(cleared, None);
}
