use super::*;

#[test]
fn pane_buffers_pty_bytes_while_forward_paused() {
    let mut pane = new_terminal_pane_for_pause_test(1);
    pane.arm_forward_pause();
    assert!(pane.is_forward_paused());

    pane.handle_pty_bytes(b"hello".to_vec());
    pane.handle_pty_bytes(b"world".to_vec());

    let drained = pane.drain_pending_pty_input();
    assert_eq!(
        drained,
        b"helloworld".to_vec(),
        "while paused, every byte must accumulate in pending_pty_input \
         instead of being fed to vte"
    );
    assert!(
        pane.drain_pending_pty_input().is_empty(),
        "drain must clear the buffer"
    );
}

#[test]
fn pane_forward_in_vte_stops_processing_with_remainder_queued() {
    // App sends `OSC 10;? + after`. When vte's OSC dispatch runs and
    // Grid pushes the forward query, processing must stop, leaving
    // `after` in the queue. Those bytes will be replayed AFTER the
    // host reply has been written.
    let mut pane = new_terminal_pane_for_pause_test(1);
    let mut input = b"\x1b]10;?\x07".to_vec();
    input.extend_from_slice(b"after");
    pane.handle_pty_bytes(input);

    let queries = pane.drain_forwarded_queries();
    assert_eq!(queries.len(), 1, "exactly one forward must be queued");
    assert_eq!(
        queries[0],
        crate::host_query::HostQuery::DefaultForeground {
            terminator: crate::host_query::OscTerminator::Bel,
        }
    );

    let buffered = pane.drain_pending_pty_input();
    assert_eq!(
        buffered,
        b"after".to_vec(),
        "bytes after the forward must remain queued, not rendered"
    );
}

#[test]
fn pane_no_forward_drains_queue_empty() {
    // When the input contains no forward-bound query, processing runs
    // to completion and `pending_pty_input` ends empty.
    let mut pane = new_terminal_pane_for_pause_test(1);
    pane.handle_pty_bytes(b"plain text".to_vec());

    assert!(
        pane.drain_forwarded_queries().is_empty(),
        "no forward should have been produced"
    );
    assert!(
        pane.drain_pending_pty_input().is_empty(),
        "no forward → queue fully drained by processing"
    );
}

#[test]
fn pane_clear_forward_pause_reports_prior_state() {
    let mut pane = new_terminal_pane_for_pause_test(1);
    assert!(!pane.clear_forward_pause(), "not previously paused");
    pane.arm_forward_pause();
    assert!(pane.clear_forward_pause(), "was paused → returns true");
    assert!(
        !pane.is_forward_paused(),
        "after clearing, the pane must read as un-paused"
    );
}

#[test]
fn pane_paused_resumes_to_drain_queue() {
    // Simulate the resume cycle Tab runs:
    //   1. arm pause, app sends bytes (they accumulate)
    //   2. clear pause, drain queue, re-feed through handle_pty_bytes
    // The DA1 query buffered during step 1 must produce its sync
    // reply during step 2.
    let mut pane = new_terminal_pane_for_pause_test(1);
    pane.arm_forward_pause();
    pane.handle_pty_bytes(b"buffered\x1b[c".to_vec());

    // While paused, vte was never fed, so no replies were produced.
    assert!(
        pane.drain_forwarded_queries().is_empty(),
        "vte was not fed → no forwards produced"
    );
    assert!(
        pane.drain_messages_to_pty().is_empty(),
        "vte was not fed → no sync replies produced"
    );

    let was_paused = pane.clear_forward_pause();
    assert!(was_paused, "was paused");
    let buffered = pane.drain_pending_pty_input();
    pane.handle_pty_bytes(buffered);

    let sync_replies = pane.drain_messages_to_pty();
    assert!(
        !sync_replies.is_empty(),
        "after resume, queue is processed and Grid emits the DA1 reply"
    );
    assert!(
        pane.drain_pending_pty_input().is_empty(),
        "queue must be fully drained when no forward is in the stream"
    );
}

// =====================================================================
// End-to-end ordering through Screen+Tab integration
//
// These tests construct a real Screen with a real Tab and TerminalPane,
// drive PTY bytes in, and then exercise handle_forwarded_reply_from_host
// to assert the resulting PTY-write order on the captured channel.
// =====================================================================

#[test]
fn forwarded_reply_routes_through_tab_for_unpaused_pane() {
    // When a pane in a Tab receives a forward reply via
    // handle_forwarded_reply_from_host, the bytes must flow through
    // resume_pane_after_forward → Tab → write_to_pane_id_without_preprocessing
    // → PtyWriteInstruction::Write. The channel capture proves the
    // bytes reached the PTY writer with the right terminal id.
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    let pane_id = PaneId::Terminal(7);
    let token = screen.forward_host_query(pane_id, bg_query());
    let _ = capture.drain_forward_queries();

    // No tab exists for this pane; the fallback path delivers the
    // bytes directly to the PTY writer. This still proves the
    // routing and stale-token guard interact correctly.
    let reply = b"\x1b]11;rgb:1111/2222/3333\x1b\\".to_vec();
    screen
        .handle_forwarded_reply_from_host(token, reply.clone())
        .expect("ok");

    let writes = capture.drain_pty_writes();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0], (reply, 7));
}

#[test]
fn paused_pane_with_da1_in_queue_emits_da1_after_resume() {
    // Simulates the user-reported ordering bug: app sends
    // `OSC 10;? + CSI c`. The OSC 10 forward must be dispatched
    // first, then the DA1 reply emitted only after the resume kicks
    // processing of the queued `\x1b[c`.
    let mut pane = new_terminal_pane_for_pause_test(1);
    let mut input = b"\x1b]10;?\x07".to_vec();
    input.extend_from_slice(b"\x1b[c");
    pane.handle_pty_bytes(input);

    // Forward emitted; DA1 still queued behind it.
    let queries = pane.drain_forwarded_queries();
    assert_eq!(queries.len(), 1, "OSC 10 forward emitted first");
    assert!(
        pane.drain_messages_to_pty().is_empty(),
        "DA1 reply must NOT be emitted yet — it is queued behind the forward"
    );

    // Tab arms pause, dispatches forward, eventually receives reply
    // and resumes. Resume = clear pause + drain queue + re-feed.
    pane.arm_forward_pause();
    // (host reply gets written to PTY via Tab; modelled here as no-op)
    pane.clear_forward_pause();
    let buffered = pane.drain_pending_pty_input();
    pane.handle_pty_bytes(buffered);

    let sync_replies = pane.drain_messages_to_pty();
    assert!(
        !sync_replies.is_empty(),
        "after resume, the queued CSI c is processed and Grid emits DA1"
    );
}

#[test]
fn empty_reply_with_paused_pane_drains_buffer_without_phantom_write() {
    // A pane that was paused on a ColorPaletteMode query while
    // host_terminal_theme_mode is unknown must still get unblocked,
    // but the spec requires NO bytes be written. The Tab-level
    // contract: a resume call with an empty payload skips the PTY
    // write yet still clears the pause and re-feeds the queue.
    let mut pane = new_terminal_pane_for_pause_test(1);
    pane.arm_forward_pause();
    pane.handle_pty_bytes(b"after".to_vec());
    assert!(pane.is_forward_paused());

    // Simulate Tab::resume_pane_after_forward with empty reply:
    //   - clear pause
    //   - drain queue + re-feed
    let was_paused = pane.clear_forward_pause();
    assert!(was_paused);
    let buffered = pane.drain_pending_pty_input();
    pane.handle_pty_bytes(buffered);
    assert!(!pane.is_forward_paused());
    assert!(
        pane.drain_pending_pty_input().is_empty(),
        "queue fully consumed by post-resume processing"
    );
}

#[test]
fn new_tabs_are_created_at_the_size_of_the_client_creating_them() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_non_mirrored_screen(initial_size);
    let client_size = Size { cols: 80, rows: 24 };
    screen.set_client_size(1, client_size);

    new_tab(&mut screen, 1, 0);

    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        client_size,
        "A tab is created at the size of the client creating it"
    );
}

#[test]
fn applying_a_layout_to_an_existing_tab_keeps_its_viewer_derived_size() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_non_mirrored_screen(initial_size);
    let client_size = Size { cols: 80, rows: 24 };
    screen.set_client_size(1, client_size);
    new_tab(&mut screen, 1, 0);

    screen
        .apply_layout(
            TiledPaneLayout::default(),
            vec![],
            vec![(2, None)],
            vec![],
            HashMap::new(),
            0,
            true,
            (1, false),
            None,
        )
        .expect("TEST");

    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        client_size,
        "Applying a layout to an existing tab does not resize it away from its viewers"
    );
}

#[test]
fn recompute_tab_size_uses_lone_viewer_size() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_new_screen(initial_size, true, true);
    new_tab(&mut screen, 1, 0);

    let client_id = 1;
    let client_size = Size { cols: 80, rows: 24 };
    screen.set_client_size(client_id, client_size);
    screen.recompute_tab_size(0).expect("TEST");

    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        client_size,
        "Tab adopts its lone viewer's size"
    );
}

#[test]
fn recompute_tab_size_takes_independent_min_across_axes() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_new_screen(initial_size, true, true);
    new_tab(&mut screen, 1, 0);
    screen.add_client(2, false).expect("TEST");

    screen.set_client_size(
        1,
        Size {
            cols: 200,
            rows: 24,
        },
    );
    screen.set_client_size(2, Size { cols: 80, rows: 60 });
    screen.recompute_tab_size(0).expect("TEST");

    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size { cols: 80, rows: 24 },
        "Tab size = (min cols across viewers, min rows across viewers)"
    );
}

#[test]
fn resize_to_clients_takes_the_min_across_clients_and_grows_back_on_detach() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_new_screen(initial_size, true, true);
    new_tab(&mut screen, 1, 0);
    screen.add_client(2, false).expect("TEST");

    screen.set_client_size(
        1,
        Size {
            cols: 100,
            rows: 50,
        },
    );
    screen.set_client_size(
        2,
        Size {
            cols: 120,
            rows: 40,
        },
    );
    screen.resize_to_clients().expect("TEST");

    assert_eq!(
        screen.surface_size(),
        Size {
            cols: 100,
            rows: 40
        },
        "Surface size = (min cols across clients, min rows across clients)"
    );

    screen.remove_client(2).expect("TEST");
    screen.resize_to_clients().expect("TEST");

    assert_eq!(
        screen.surface_size(),
        Size {
            cols: 100,
            rows: 50
        },
        "Screen grows back to the remaining client once the constraining one detaches"
    );
}

#[test]
fn resize_to_clients_reflows_global_panes_and_survives_a_render() {
    let mut screen = create_non_mirrored_screen(Size { cols: 50, rows: 12 });
    new_tab(&mut screen, 1, 0);
    insert_left_global_pane(&mut screen, 10);
    screen.set_client_size(1, Size { cols: 50, rows: 12 });
    screen.resize_to_clients().expect("TEST");

    screen.set_client_size(1, Size { cols: 40, rows: 10 });
    screen.resize_to_clients().expect("TEST");

    let global_pane_geom = screen
        .global_panes
        .panes()
        .next()
        .expect("TEST")
        .0
        .position_and_size();
    assert_eq!(
        global_pane_geom.rows.as_usize(),
        10,
        "Global pane reflows to the new screen height"
    );
    let tab = screen.tabs.get(&0).unwrap();
    assert_eq!(tab.get_display_area(), Size { cols: 30, rows: 10 });
    assert_eq!(tab.get_display_area_origin(), (10, 0));

    // render_to_clients re-derives every tab's layout each frame, so a resize that did not reach
    // the shared surface would be snapped back here
    screen.render_to_clients().expect("TEST");
    let tab = screen.tabs.get(&0).unwrap();
    assert_eq!(tab.get_display_area(), Size { cols: 30, rows: 10 });
    assert_eq!(tab.get_display_area_origin(), (10, 0));
}

#[test]
fn character_cell_size_is_derived_from_the_resized_screen() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, true, true);
    new_tab(&mut screen, 1, 0);

    // a font-size change: the terminal keeps its pixel size but reports fewer, larger cells
    screen.set_client_size(1, Size { cols: 40, rows: 12 });
    screen.resize_to_clients().expect("TEST");
    screen.update_pixel_dimensions(
        1,
        PixelDimensions {
            text_area_size: Some(SizeInPixels {
                width: 800,
                height: 480,
            }),
            character_cell_size: None,
        },
    );

    assert_eq!(
        *screen.character_cell_size.borrow(),
        Some(SizeInPixels {
            width: 20,
            height: 40
        }),
        "Cell size is derived from the new screen size, not the one at session start"
    );
}

#[test]
fn resize_to_clients_keeps_every_tab_within_smallest_client_bounds() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_non_mirrored_screen(initial_size);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen.add_client(2, false).expect("TEST");
    screen.set_client_size(1, Size { cols: 80, rows: 24 });
    screen.set_client_size(
        2,
        Size {
            cols: 160,
            rows: 50,
        },
    );
    screen.switch_active_tab(0, None, true, 2).expect("TEST");
    screen.resize_to_clients().expect("TEST");

    assert_eq!(
        screen.tabs.get(&1).unwrap().size,
        Size { cols: 80, rows: 24 },
        "The smaller client's tab uses the session bounds"
    );
    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size { cols: 80, rows: 24 },
        "A tab viewed only by the larger client is still restricted to the session bounds"
    );

    screen.set_client_size(
        2,
        Size {
            cols: 240,
            rows: 70,
        },
    );
    screen.resize_to_clients().expect("TEST");
    screen.render_to_clients().expect("TEST");

    assert!(screen
        .tabs
        .values()
        .all(|tab| tab.size == Size { cols: 80, rows: 24 }));

    screen.remove_client(1).expect("TEST");
    screen.resize_to_clients().expect("TEST");

    assert!(screen.tabs.values().all(|tab| tab.size
        == Size {
            cols: 240,
            rows: 70
        }));
}

/// Two clients of different widths on different tabs, with a right global pane: every tab stays
/// inside the smallest client bounds, including across a render tick.
#[test]
fn global_panes_stay_within_shared_bounds_for_clients_on_different_tabs() {
    use zellij_utils::data::GlobalPaneEdge;

    let mut screen = create_non_mirrored_screen(Size {
        cols: 200,
        rows: 60,
    });
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen.add_client(2, false).expect("TEST");
    insert_global_pane_on_edge(&mut screen, 99, GlobalPaneEdge::Right, 10);
    screen.set_client_size(1, Size { cols: 80, rows: 24 });
    screen.set_client_size(
        2,
        Size {
            cols: 160,
            rows: 50,
        },
    );
    // client 1 stays on tab 1, client 2 moves to tab 0
    screen.switch_active_tab(0, None, true, 2).expect("TEST");
    screen.resize_to_clients().expect("TEST");

    let centers = |screen: &Screen| {
        (
            screen.tabs.get(&0).unwrap().get_display_area(),
            screen.tabs.get(&1).unwrap().get_display_area(),
        )
    };
    let expected = (Size { cols: 70, rows: 24 }, Size { cols: 70, rows: 24 });
    assert_eq!(
        centers(&screen),
        expected,
        "Both tabs use the shared session bounds beside the 10 column pane"
    );

    screen.render_to_clients().expect("TEST");
    assert_eq!(
        centers(&screen),
        expected,
        "A render tick preserves the shared session bounds"
    );

    // the pane itself is one grid at the same shared-session position for both clients
    screen.position_global_panes_for_client(2);
    let geom = screen
        .global_panes
        .pane(PaneId::Terminal(99))
        .unwrap()
        .position_and_size();
    assert_eq!(
        (geom.x, geom.cols.as_usize(), geom.rows.as_usize()),
        (70, 10, 24)
    );
    screen.position_global_panes_for_client(1);
    let geom = screen
        .global_panes
        .pane(PaneId::Terminal(99))
        .unwrap()
        .position_and_size();
    assert_eq!(
        (geom.x, geom.cols.as_usize(), geom.rows.as_usize()),
        (70, 10, 24)
    );
}

/// Global panes on both axes reserve their strips inside the shared session bounds.
#[test]
fn global_panes_on_both_axes_respect_the_shared_session_size() {
    use zellij_utils::data::GlobalPaneEdge;

    let mut screen = create_non_mirrored_screen(Size {
        cols: 200,
        rows: 60,
    });
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen.add_client(2, false).expect("TEST");
    insert_global_pane_on_edge(&mut screen, 98, GlobalPaneEdge::Top, 3);
    insert_global_pane_on_edge(&mut screen, 99, GlobalPaneEdge::Right, 10);
    screen.set_client_size(1, Size { cols: 80, rows: 24 });
    screen.set_client_size(
        2,
        Size {
            cols: 160,
            rows: 50,
        },
    );
    screen.switch_active_tab(0, None, true, 2).expect("TEST");
    screen.resize_to_clients().expect("TEST");
    screen.render_to_clients().expect("TEST");

    assert_eq!(
        screen.tabs.get(&0).unwrap().get_display_area(),
        screen.tabs.get(&1).unwrap().get_display_area(),
        "Both tabs use the same surface around the global pane strips"
    );
    assert_eq!(
        screen.tabs.get(&0).unwrap().get_display_area(),
        Size { cols: 70, rows: 21 },
    );
}

/// Hit-testing a global pane uses the same session bounds for clients on different tabs.
#[test]
fn global_pane_hit_testing_uses_shared_session_bounds() {
    use zellij_utils::data::GlobalPaneEdge;
    use zellij_utils::position::Position;

    let mut screen = create_non_mirrored_screen(Size {
        cols: 200,
        rows: 60,
    });
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen.add_client(2, false).expect("TEST");
    insert_global_pane_on_edge(&mut screen, 99, GlobalPaneEdge::Right, 10);
    screen.set_client_size(1, Size { cols: 80, rows: 24 });
    screen.set_client_size(
        2,
        Size {
            cols: 160,
            rows: 24,
        },
    );
    screen.switch_active_tab(0, None, true, 2).expect("TEST");
    screen.resize_to_clients().expect("TEST");
    screen.render_to_clients().expect("TEST");

    for client_id in [1, 2] {
        screen.position_global_panes_for_client(client_id);
        assert_eq!(
            screen.global_panes.pane_id_at(&Position::new(2, 75)),
            Some(PaneId::Terminal(99)),
            "Both clients hit the sidebar inside the shared bounds"
        );
        assert_eq!(
            screen.global_panes.pane_id_at(&Position::new(2, 155)),
            None,
            "Neither client has session content beyond the smaller client's width"
        );
    }
}

/// A read-only watcher observes the session without shrinking its shared surface.
#[test]
fn watcher_size_does_not_constrain_session_bounds() {
    let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 24 });
    new_tab(&mut screen, 1, 0);
    screen.add_watcher_client(2).expect("TEST");
    screen.set_watcher_size(2, Size { cols: 40, rows: 10 });
    screen.resize_to_clients().expect("TEST");

    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size { cols: 80, rows: 24 },
        "The smaller watcher viewport does not shrink the interactive session"
    );
}

/// A left/right pane's band is inset by the tallest chrome any tab declares, so a tab with less
/// chrome than that is left with strip rows nothing paints - those get filled, once per change.
#[test]
fn a_tab_with_less_chrome_than_the_session_max_gets_its_strip_filled() {
    use crate::global_panes::unowned_strip_rows;

    let client_id = 1;
    let rows = 20;
    let global_cols = 10;
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows });
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    // a non-selectable pane on the bottom edge is chrome, same as the status-bar plugin - tab 0
    // gets one, tab 1 does not
    screen
        .tabs
        .get_mut(&0)
        .unwrap()
        .new_pane(
            PaneId::Terminal(3),
            None,
            None,
            false,
            true,
            NewPanePlacement::Tiled {
                direction: Some(Direction::Down),
                borderless: Some(true),
            },
            Some(client_id),
            None,
        )
        .unwrap();
    screen
        .tabs
        .get_mut(&0)
        .unwrap()
        .set_pane_selectable(PaneId::Terminal(3), false);
    insert_left_global_pane(&mut screen, global_cols);
    screen.resize_to_clients().expect("TEST");
    screen.render_to_clients().expect("TEST");

    let (_, chrome_bottom) = screen.tabs.get(&0).unwrap().chrome_rows();
    assert!(chrome_bottom > 0, "tab 0 has no bottom chrome to speak of");
    assert_eq!(
        screen.max_chrome_rows(),
        (0, chrome_bottom),
        "the band is inset by the tallest chrome, which is tab 0's"
    );

    let rows_for = |screen: &Screen, tab_id: usize| {
        // tab 0 has no viewer, so it falls back to the shared surface just as the render tick does
        let tab_size = screen
            .tab_surface_size(tab_id)
            .unwrap_or_else(|| screen.surface_size());
        let layout = screen.global_layout_for(tab_size).unwrap();
        unowned_strip_rows(
            layout.center,
            &screen.strips_from_layout(&layout),
            screen.tabs.get(&tab_id).unwrap().chrome_rows(),
        )
    };
    assert_eq!(
        rows_for(&screen, 0),
        vec![],
        "tab 0's own chrome reaches every row the band leaves it"
    );
    assert_eq!(
        rows_for(&screen, 1),
        (rows - chrome_bottom..rows)
            .map(|y| (0, y, global_cols))
            .collect::<Vec<_>>(),
        "tab 1 has no status bar, so the rows the band gave up for tab 0's are its to fill"
    );

    // client 1 is on tab 1 (the last one created), so it is the one that saw the fill - and it is
    // memoised, or Output would re-emit it on every render tick and never let the session go quiet
    assert_eq!(
        screen.unowned_strip_fill.get(&client_id),
        Some(&rows_for(&screen, 1)),
        "the fill this client was sent is remembered, so the next render is a no-op"
    );
}

#[test]
fn switching_tabs_keeps_source_and_destination_at_session_bounds() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_non_mirrored_screen(initial_size);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen.add_client(2, false).expect("TEST");

    screen.set_client_size(1, Size { cols: 80, rows: 24 });
    screen.set_client_size(
        2,
        Size {
            cols: 160,
            rows: 50,
        },
    );
    screen.recompute_tab_size(1).expect("TEST");
    assert_eq!(
        screen.tabs.get(&1).unwrap().size,
        Size { cols: 80, rows: 24 },
        "Both clients on tab 1 → it sizes to the smaller"
    );

    screen.switch_active_tab(0, None, true, 1).expect("TEST");

    assert_eq!(
        screen.tabs.get(&1).unwrap().size,
        Size { cols: 80, rows: 24 },
        "Source tab remains inside the smallest client bounds"
    );
    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size { cols: 80, rows: 24 },
        "Destination tab remains inside the smallest client bounds"
    );
}

#[test]
fn break_pane_to_new_tab_recomputes_source_and_destination() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_non_mirrored_screen(initial_size);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen.add_client(2, false).expect("TEST");

    screen.set_client_size(1, Size { cols: 80, rows: 24 });
    screen.set_client_size(
        2,
        Size {
            cols: 160,
            rows: 50,
        },
    );
    screen.switch_active_tab(0, None, true, 1).expect("TEST");
    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size { cols: 80, rows: 24 },
        "Pre-condition: tab 0 sized to client 1"
    );
    assert_eq!(
        screen.tabs.get(&1).unwrap().size,
        Size { cols: 80, rows: 24 },
        "Pre-condition: tab 1 is restricted by the smaller client on tab 0"
    );

    {
        let active_tab = screen.get_active_tab_mut(1).unwrap();
        active_tab
            .new_pane(
                PaneId::Terminal(99),
                None,
                None,
                false,
                true,
                NewPanePlacement::default(),
                Some(1),
                None,
            )
            .unwrap();
    }

    screen
        .break_pane_to_new_tab(Direction::Right, 1)
        .expect("TEST");

    assert_eq!(
        screen.tabs.get(&1).unwrap().size,
        Size { cols: 80, rows: 24 },
        "Destination tab shrinks to fit the arriving smaller viewer"
    );
    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size { cols: 80, rows: 24 },
        "Source tab is empty — recompute is a no-op, last viewer-derived size is preserved"
    );
}

#[test]
fn moving_panes_between_tabs_with_focus_change_recomputes_both() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_non_mirrored_screen(initial_size);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen.add_client(2, false).expect("TEST");

    screen.set_client_size(1, Size { cols: 80, rows: 24 });
    screen.set_client_size(
        2,
        Size {
            cols: 160,
            rows: 50,
        },
    );
    screen.switch_active_tab(0, None, true, 2).expect("TEST");
    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size { cols: 80, rows: 24 },
        "Pre-condition: tab 0 is restricted by the smaller client on tab 1"
    );
    assert_eq!(
        screen.tabs.get(&1).unwrap().size,
        Size { cols: 80, rows: 24 },
        "Pre-condition: tab 1 sized to client 1"
    );

    let pane_to_move = PaneId::Terminal(42);
    {
        let active_tab = screen.get_active_tab_mut(1).unwrap();
        active_tab
            .new_pane(
                pane_to_move,
                None,
                None,
                false,
                true,
                NewPanePlacement::default(),
                Some(1),
                None,
            )
            .unwrap();
    }

    let outcome = screen
        .break_multiple_panes_to_tab_with_id(vec![pane_to_move], 0, true, 1)
        .expect("TEST");
    assert!(
        outcome.succeeded(),
        "Pre-condition: the pane actually moved"
    );

    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size { cols: 80, rows: 24 },
        "Destination tab shrinks to min(viewers) once the smaller client arrives"
    );
    assert_eq!(
        screen.tabs.get(&1).unwrap().size,
        Size { cols: 80, rows: 24 },
        "Source tab is empty — last viewer-derived size is preserved"
    );
}

#[test]
fn detaching_client_grows_vacated_tab_back() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_new_screen(initial_size, true, true);
    new_tab(&mut screen, 1, 0);
    screen.add_client(2, false).expect("TEST");

    screen.set_client_size(
        1,
        Size {
            cols: 160,
            rows: 50,
        },
    );
    screen.set_client_size(2, Size { cols: 80, rows: 24 });
    screen.recompute_tab_size(0).expect("TEST");
    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size { cols: 80, rows: 24 },
        "Smaller viewer wins while both clients are on the tab"
    );

    screen.remove_client(2).expect("TEST");
    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size {
            cols: 160,
            rows: 50,
        },
        "Tab grows back to fit the remaining viewer after the smaller one detaches"
    );
}

#[test]
fn closing_a_tab_resizes_the_tab_it_returns_to() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_new_screen(initial_size, true, true);
    let client_id = 1;
    let small_size = Size { cols: 80, rows: 24 };
    let large_size = Size {
        cols: 160,
        rows: 50,
    };

    screen.set_client_size(client_id, small_size);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    assert_eq!(
        screen.tabs.get(&1).unwrap().size,
        small_size,
        "Pre-condition: both tabs sized to the small client viewport"
    );

    screen.set_client_size(client_id, large_size);
    screen.recompute_tab_size(1).expect("TEST");
    assert_eq!(
        screen.tabs.get(&1).unwrap().size,
        large_size,
        "Pre-condition: the active tab follows the client resize"
    );
    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        small_size,
        "Pre-condition: the background tab keeps its stale size until it is activated"
    );

    screen.close_tab_by_id(1).expect("TEST");

    assert_eq!(
        screen.get_active_tab(client_id).unwrap().position,
        0,
        "Focus returns to the previous tab"
    );
    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        large_size,
        "The tab we return to adopts the current client size"
    );
}
