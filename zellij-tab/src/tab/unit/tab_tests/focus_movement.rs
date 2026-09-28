use super::*;

#[test]
pub fn move_focus_down() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));

    tab.horizontal_split(new_pane_id, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();
    tab.move_focus_down(crate::ClientId::new(1)).unwrap();

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().y(),
        10,
        "Active pane is the bottom one"
    );
}

#[test]
pub fn move_focus_down_to_the_most_recently_used_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));

    tab.horizontal_split(new_pane_id_1, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.vertical_split(new_pane_id_2, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.vertical_split(new_pane_id_3, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();
    tab.move_focus_down(crate::ClientId::new(1)).unwrap();

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().y(),
        10,
        "Active pane y position"
    );
    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().x(),
        91,
        "Active pane x position"
    );
}

#[test]
pub fn move_focus_up() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));

    tab.horizontal_split(new_pane_id, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().y(),
        0,
        "Active pane is the top one"
    );
}

#[test]
pub fn move_focus_up_to_the_most_recently_used_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));

    tab.horizontal_split(new_pane_id_1, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();
    tab.vertical_split(new_pane_id_2, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.vertical_split(new_pane_id_3, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_focus_down(crate::ClientId::new(1)).unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().y(),
        0,
        "Active pane y position"
    );
    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().x(),
        91,
        "Active pane x position"
    );
}

#[test]
pub fn move_focus_left() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));

    tab.vertical_split(new_pane_id, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_focus_left(crate::ClientId::new(1)).unwrap();

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().x(),
        0,
        "Active pane is the left one"
    );
}

#[test]
pub fn move_focus_left_to_the_most_recently_used_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));

    tab.vertical_split(new_pane_id_1, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_focus_left(crate::ClientId::new(1)).unwrap();
    tab.horizontal_split(new_pane_id_2, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.horizontal_split(new_pane_id_3, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_focus_right(crate::ClientId::new(1)).unwrap();
    tab.move_focus_left(crate::ClientId::new(1)).unwrap();

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().y(),
        15,
        "Active pane y position"
    );
    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().x(),
        0,
        "Active pane x position"
    );
}

#[test]
pub fn move_focus_right() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));

    tab.vertical_split(new_pane_id, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_focus_left(crate::ClientId::new(1)).unwrap();
    tab.move_focus_right(crate::ClientId::new(1)).unwrap();

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().x(),
        61,
        "Active pane is the right one"
    );
}

#[test]
pub fn move_focus_right_to_the_most_recently_used_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));

    tab.vertical_split(new_pane_id_1, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.horizontal_split(new_pane_id_2, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.horizontal_split(new_pane_id_3, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_focus_left(crate::ClientId::new(1)).unwrap();
    tab.move_focus_right(crate::ClientId::new(1)).unwrap();

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().y(),
        15,
        "Active pane y position"
    );
    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().x(),
        61,
        "Active pane x position"
    );
}

/// The pty writer is told a moved pane's new size. Its channel being gone - the thread has
/// exited, the session is on its way down - is logged, not fatal: the geometry has already
/// moved, and a keystroke racing teardown must not take the screen thread with it.
#[test]
pub fn moving_a_pane_after_the_pty_writer_exited_swaps_it_without_panicking() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let client_id = crate::ClientId::new(1);
    // Send errors surface, as in production, rather than being swallowed as the test default
    // does, which would hide the case under test. Every other thread keeps listening, so the pty
    // writer's is the only send that can fail.
    let (to_screen, _screen_receiver) = unbounded();
    let (to_pty, _pty_receiver) = unbounded();
    let (to_plugin, _plugin_receiver) = unbounded();
    let (to_server, _server_receiver) = unbounded();
    let (to_pty_writer, pty_writer_receiver) = unbounded();
    let (to_background_jobs, _background_jobs_receiver) = unbounded();
    let senders = ThreadSenders {
        to_screen: Some(SenderWithContext::new(to_screen)),
        to_pty: Some(SenderWithContext::new(to_pty)),
        to_plugin: Some(SenderWithContext::new(to_plugin)),
        to_server: Some(SenderWithContext::new(to_server)),
        to_pty_writer: Some(SenderWithContext::new(to_pty_writer)),
        to_background_jobs: Some(SenderWithContext::new(to_background_jobs)),
        ..ThreadSenders::default()
    };
    let mut tab = create_new_tab_with_senders(size, stacked_resize, senders);
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    tab.horizontal_split(new_pane_id, None, client_id, None, None)
        .unwrap();
    tab.move_focus_up(client_id).unwrap();
    drop(pty_writer_receiver);

    tab.move_active_pane_down(client_id);

    assert_eq!(
        tab.get_active_pane(client_id).unwrap().y(),
        10,
        "the pane moved to the bottom"
    );
    assert_eq!(
        tab.get_active_pane(client_id).unwrap().pid(),
        PaneId::Terminal(crate::TerminalId::new(1)),
        "the same pane is still focused"
    );
}

#[test]
pub fn move_active_pane_down() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));

    tab.horizontal_split(new_pane_id, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();
    tab.move_active_pane_down(crate::ClientId::new(1));

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().y(),
        10,
        "Active pane is the bottom one"
    );
    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().pid(),
        PaneId::Terminal(crate::TerminalId::new(1)),
        "Active pane is the bottom one"
    );
}

#[test]
pub fn move_active_pane_down_to_the_most_recently_used_position() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));

    tab.horizontal_split(new_pane_id_1, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.vertical_split(new_pane_id_2, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.vertical_split(new_pane_id_3, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();
    tab.move_active_pane_down(crate::ClientId::new(1));

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().y(),
        10,
        "Active pane y position"
    );
    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().x(),
        91,
        "Active pane x position"
    );
    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().pid(),
        PaneId::Terminal(crate::TerminalId::new(1)),
        "Active pane PaneId"
    );
}

#[test]
pub fn move_active_pane_up() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));

    tab.horizontal_split(new_pane_id, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_active_pane_up(crate::ClientId::new(1));

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().y(),
        0,
        "Active pane is the top one"
    );
    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().pid(),
        PaneId::Terminal(crate::TerminalId::new(2)),
        "Active pane is the top one"
    );
}

#[test]
pub fn move_active_pane_up_to_the_most_recently_used_position() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));

    tab.horizontal_split(new_pane_id_1, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();
    tab.vertical_split(new_pane_id_2, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.vertical_split(new_pane_id_3, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_focus_down(crate::ClientId::new(1)).unwrap();
    tab.move_active_pane_up(crate::ClientId::new(1));

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().y(),
        0,
        "Active pane y position"
    );
    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().x(),
        91,
        "Active pane x position"
    );

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().pid(),
        PaneId::Terminal(crate::TerminalId::new(2)),
        "Active pane PaneId"
    );
}

#[test]
pub fn move_active_pane_left() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));

    tab.vertical_split(new_pane_id, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_active_pane_left(crate::ClientId::new(1));

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().x(),
        0,
        "Active pane is the left one"
    );
    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().pid(),
        PaneId::Terminal(crate::TerminalId::new(2)),
        "Active pane is the left one"
    );
}

#[test]
pub fn move_active_pane_left_to_the_most_recently_used_position() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));

    tab.vertical_split(new_pane_id_1, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_focus_left(crate::ClientId::new(1)).unwrap();
    tab.horizontal_split(new_pane_id_2, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.horizontal_split(new_pane_id_3, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_focus_right(crate::ClientId::new(1)).unwrap();
    tab.move_active_pane_left(crate::ClientId::new(1));

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().y(),
        15,
        "Active pane y position"
    );
    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().x(),
        0,
        "Active pane x position"
    );

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().pid(),
        PaneId::Terminal(crate::TerminalId::new(2)),
        "Active pane PaneId"
    );
}

#[test]
pub fn move_active_pane_right() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));

    tab.vertical_split(new_pane_id, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_focus_left(crate::ClientId::new(1)).unwrap();
    tab.move_active_pane_right(crate::ClientId::new(1));

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().x(),
        61,
        "Active pane is the right one"
    );
    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().pid(),
        PaneId::Terminal(crate::TerminalId::new(1)),
        "Active pane is the right one"
    );
}

#[test]
pub fn move_active_pane_right_to_the_most_recently_used_position() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));

    tab.vertical_split(new_pane_id_1, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.horizontal_split(new_pane_id_2, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.horizontal_split(new_pane_id_3, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_focus_left(crate::ClientId::new(1)).unwrap();
    tab.move_active_pane_right(crate::ClientId::new(1));

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().y(),
        15,
        "Active pane y position"
    );
    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().x(),
        61,
        "Active pane x position"
    );
    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().pid(),
        PaneId::Terminal(crate::TerminalId::new(1)),
        "Active pane Paneid"
    );
}

/// A floating pane moved to its next slot repaints what it was covering. The directional moves
/// always marked the floating layout and forced a render; the "next slot" entry points moved
/// the pane and left the tiled panes under it unmarked, so what it had covered stayed on screen.
#[test]
pub fn moving_a_floating_pane_to_its_next_slot_repaints_the_panes_under_it() {
    let client_id = crate::ClientId::new(1);
    let tiled_pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    let floating_pane_id = PaneId::Terminal(crate::TerminalId::new(3));
    type NextSlotMove = fn(&mut Tab, crate::ClientId, PaneId);
    let moves: [(&str, NextSlotMove); 5] = [
        ("move_active_pane", |tab, client_id, _| {
            tab.move_active_pane(client_id)
        }),
        ("move_active_pane_backwards", |tab, client_id, _| {
            tab.move_active_pane_backwards(client_id)
        }),
        ("move_pane", |tab, _, pane_id| tab.move_pane(pane_id)),
        ("move_pane_by_pane_id", |tab, _, pane_id| {
            tab.move_pane_by_pane_id(pane_id, None)
        }),
        ("move_pane_backwards_by_pane_id", |tab, _, pane_id| {
            tab.move_pane_backwards_by_pane_id(pane_id)
        }),
    ];
    for (name, move_pane) in moves {
        let mut tab = create_tab_with_two_floating_panes();
        let mut output = zellij_terminal::output::Output::default();
        tab.render(&mut output, Some(client_id)).unwrap();
        assert!(
            !tab.tiled_panes
                .get_pane(tiled_pane_id)
                .unwrap()
                .should_render(),
            "{name}: the tiled pane is painted before the move"
        );
        tab.swap_layouts.reset_floating_damage();

        move_pane(&mut tab, client_id, floating_pane_id);

        assert!(
            tab.swap_layouts.is_floating_damaged(),
            "{name}: the floating layout is marked as changed"
        );
        assert!(
            tab.tiled_panes
                .get_pane(tiled_pane_id)
                .unwrap()
                .should_render(),
            "{name}: the tiled pane under the moved one is repainted"
        );
    }
}

/// A floating pane moves while a tiled pane is fullscreen underneath it. The gate against a
/// tiled fullscreen belongs to the tiled layer; the "next slot" entry points applied it before
/// choosing a layer and refused the move the directional ones allowed.
#[test]
pub fn a_floating_pane_moves_to_its_next_slot_while_a_tiled_pane_is_fullscreen() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = crate::ClientId::new(1);
    let mut tab = create_new_tab(size, false);
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        client_id,
        None,
        None,
    )
    .unwrap();
    tab.toggle_active_pane_fullscreen(client_id);
    for id in [3, 4] {
        tab.new_floating_pane(
            PaneId::Terminal(crate::TerminalId::new(id)),
            None,
            None,
            false,
            true,
            None,
            None,
        )
        .unwrap();
    }
    assert!(
        tab.tiled_panes.fullscreen_is_active() && tab.floating_panes.panes_are_visible(),
        "the state under test: floating panes shown over a tiled fullscreen"
    );
    let active_pane_id = tab
        .floating_panes
        .active_pane_id(client_id)
        .expect("a floating pane is focused");
    let geom_before = floating_pane_geom(&tab, active_pane_id);

    tab.move_active_pane(client_id);

    assert_ne!(
        floating_pane_geom(&tab, active_pane_id),
        geom_before,
        "the floating pane moved"
    );
}

/// Moving the active pane of a client the tab has never seen moves nothing. It used to reach
/// `TiledPanes::move_active_pane`, which unwrapped that client's active pane.
#[test]
pub fn moving_the_active_pane_of_an_unknown_client_is_a_no_op() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, false);
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    let geoms_before = tiled_pane_geoms(&tab);

    tab.move_active_pane(crate::ClientId::new(7));
    tab.move_active_pane_backwards(crate::ClientId::new(7));

    assert_eq!(tiled_pane_geoms(&tab), geoms_before, "nothing moved");
}
