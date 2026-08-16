use super::*;

#[test]
fn attaching_web_client_lands_on_the_first_tab() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    assert_eq!(
        screen.active_tab_ids.get(&1),
        Some(&1),
        "host client is on the second tab"
    );

    screen.add_client(2, true).expect("TEST");

    assert_eq!(
        screen.active_tab_ids.get(&2),
        Some(&0),
        "web client is on the first tab"
    );
}

#[test]
fn attaching_web_client_hides_the_floating_surface_of_the_first_tab() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, 0);
    screen
        .tabs
        .get_mut(&0)
        .unwrap()
        .new_floating_pane(PaneId::Terminal(2), None, None, false, true, None, None)
        .unwrap();
    assert!(screen.tabs.get(&0).unwrap().are_floating_panes_visible());

    screen.add_client(2, true).expect("TEST");

    assert!(
        !screen.tabs.get(&0).unwrap().are_floating_panes_visible(),
        "the floating surface is hidden"
    );
    assert_eq!(
        screen.get_active_pane_id(&2),
        Some(PaneId::Terminal(1)),
        "web client is focused on a tiled pane"
    );
}

#[test]
fn attaching_web_client_does_not_touch_the_floating_surface_of_other_tabs() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen
        .tabs
        .get_mut(&1)
        .unwrap()
        .new_floating_pane(PaneId::Terminal(3), None, None, false, true, None, None)
        .unwrap();

    screen.add_client(2, true).expect("TEST");

    assert!(
        screen.tabs.get(&1).unwrap().are_floating_panes_visible(),
        "the floating surface of the host tab is untouched"
    );
    assert_eq!(
        screen.get_active_pane_id(&1),
        Some(PaneId::Terminal(3)),
        "host client keeps its floating focus"
    );
}

#[test]
fn attaching_terminal_client_follows_the_host_tab() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen
        .tabs
        .get_mut(&1)
        .unwrap()
        .new_floating_pane(PaneId::Terminal(3), None, None, false, true, None, None)
        .unwrap();

    screen.add_client(2, false).expect("TEST");

    assert_eq!(
        screen.active_tab_ids.get(&2),
        Some(&1),
        "terminal client is on the host tab"
    );
    assert!(
        screen.tabs.get(&1).unwrap().are_floating_panes_visible(),
        "the floating surface remains visible"
    );
}

#[test]
fn attaching_web_watcher_follows_the_host_tab() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);

    screen.add_watcher_client(2).expect("TEST");
    screen.add_client(2, true).expect("TEST");

    assert_eq!(
        screen.active_tab_ids.get(&2),
        Some(&1),
        "watcher client mirrors the host tab"
    );
}
