//! `Tab::set_related_panes` - the gentle "these agents share a checkout" mark.
//!
//! The mark lives on the pane rather than in a set the render walks, which is what makes it
//! cost nothing and disappear with the pane. The price is that setting it is a whole-session
//! assignment rather than an insert, and these pin that it behaves like one.

use super::*;

/// Two tabs with a terminal in each, so a set aimed at one tab can be seen not to disturb
/// the other.
fn screen_with_two_tabs() -> Screen {
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    screen
}

fn is_related(screen: &mut Screen, pane_id: PaneId) -> bool {
    screen
        .with_pane(pane_id, |pane| pane.has_related_highlight())
        .expect("the pane exists")
}

fn set_related(screen: &mut Screen, pane_ids: &[PaneId]) -> bool {
    let wanted: HashSet<PaneId> = pane_ids.iter().copied().collect();
    let mut changed = false;
    for tab in screen.tabs.values_mut() {
        changed |= tab.set_related_panes(&wanted);
    }
    changed
}

#[test]
fn a_pane_in_the_set_is_marked_and_one_outside_it_is_not() {
    let mut screen = screen_with_two_tabs();
    assert!(set_related(
        &mut screen,
        &[PaneId::Terminal(TerminalId::new(1))]
    ));
    assert!(is_related(
        &mut screen,
        PaneId::Terminal(TerminalId::new(1))
    ));
    assert!(!is_related(
        &mut screen,
        PaneId::Terminal(TerminalId::new(2))
    ));
}

/// The whole point of replace-the-whole-set: the sender says what is related *now* and is
/// never asked to remember what it last said, so a pane dropping out of the set has to be
/// unmarked by the arrival of a set it is not in.
#[test]
fn a_new_set_takes_the_mark_off_everything_not_in_it() {
    let mut screen = screen_with_two_tabs();
    set_related(
        &mut screen,
        &[
            PaneId::Terminal(TerminalId::new(1)),
            PaneId::Terminal(TerminalId::new(2)),
        ],
    );

    set_related(&mut screen, &[PaneId::Terminal(TerminalId::new(2))]);
    assert!(!is_related(
        &mut screen,
        PaneId::Terminal(TerminalId::new(1))
    ));
    assert!(is_related(
        &mut screen,
        PaneId::Terminal(TerminalId::new(2))
    ));
}

/// The empty set is how a highlight is taken down - the daemon sends it whenever focus
/// leaves an agent, which is most of the time.
#[test]
fn the_empty_set_clears_every_mark() {
    let mut screen = screen_with_two_tabs();
    set_related(
        &mut screen,
        &[
            PaneId::Terminal(TerminalId::new(1)),
            PaneId::Terminal(TerminalId::new(2)),
        ],
    );

    assert!(set_related(&mut screen, &[]));
    assert!(!is_related(
        &mut screen,
        PaneId::Terminal(TerminalId::new(1))
    ));
    assert!(!is_related(
        &mut screen,
        PaneId::Terminal(TerminalId::new(2))
    ));
}

/// The daemon republishes on every pane update, so most of these say exactly what the last
/// one did. Answering "nothing moved" is what keeps that from being a repaint per update.
#[test]
fn re_sending_the_same_set_reports_no_change() {
    let mut screen = screen_with_two_tabs();
    assert!(set_related(
        &mut screen,
        &[PaneId::Terminal(TerminalId::new(1))]
    ));
    assert!(!set_related(
        &mut screen,
        &[PaneId::Terminal(TerminalId::new(1))]
    ));
    assert!(
        set_related(&mut screen, &[]),
        "clearing a set that was actually up is a change"
    );
    assert!(!set_related(&mut screen, &[]));
}

/// A pane id in the set that names no pane is simply not there - terminal ids are recycled,
/// and a mark that outlived its pane must not land on whatever holds the id next.
#[test]
fn a_pane_that_has_closed_takes_its_mark_with_it() {
    let mut screen = screen_with_two_tabs();
    set_related(&mut screen, &[PaneId::Terminal(TerminalId::new(1))]);

    let tab_id = *screen.tabs.keys().next().expect("a tab to close it in");
    screen.tabs.get_mut(&tab_id).expect("TEST").close_pane(
        PaneId::Terminal(TerminalId::new(1)),
        false,
        None,
    );
    // The id is now free. A later set that does not name it must leave whatever takes it
    // next unmarked, which it does because the flag went away with the pane.
    set_related(&mut screen, &[PaneId::Terminal(TerminalId::new(2))]);
    assert!(is_related(
        &mut screen,
        PaneId::Terminal(TerminalId::new(2))
    ));
    assert!(
        screen
            .with_pane(PaneId::Terminal(TerminalId::new(1)), |pane| pane
                .has_related_highlight())
            .is_none(),
        "the closed pane is gone, mark and all"
    );
}
