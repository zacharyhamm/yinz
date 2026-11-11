use super::*;
use yinz_panes::pane::TerminalPaneIo;
use yinz_types::internal_ids::TabId;
use yinz_types::internal_ids::TabNumber;
use yinz_types::internal_ids::TabPosition;

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
    let pane_id = PaneId::Terminal(crate::TerminalId::new(7));
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
    assert_eq!(writes[0], (reply, TerminalId::new(7)));
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
    screen
        .set_client_size(crate::ClientId::new(1), client_size)
        .unwrap();

    new_tab(&mut screen, 1, TabId::new(0));

    assert_eq!(
        screen.tabs.get(&TabId::new(0)).unwrap().size,
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
    screen
        .set_client_size(crate::ClientId::new(1), client_size)
        .unwrap();
    new_tab(&mut screen, 1, TabId::new(0));

    screen
        .apply_layout(
            TiledPaneLayout::default(),
            vec![],
            vec![(crate::TerminalId::new(2), None)],
            vec![],
            HashMap::new(),
            TabId::new(0),
            true,
            crate::ClientId::new(1),
            None,
        )
        .expect("TEST");

    assert_eq!(
        screen.tabs.get(&TabId::new(0)).unwrap().size,
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
    new_tab(&mut screen, 1, TabId::new(0));

    let client_id = ClientId::new(1);
    let client_size = Size { cols: 80, rows: 24 };
    screen.set_client_size(client_id, client_size).unwrap();
    screen.recompute_tab_size(TabId::new(0)).expect("TEST");

    assert_eq!(
        screen.tabs.get(&TabId::new(0)).unwrap().size,
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
    new_tab(&mut screen, 1, TabId::new(0));
    screen.add_client(crate::ClientId::new(2)).expect("TEST");

    screen
        .set_client_size(
            crate::ClientId::new(1),
            Size {
                cols: 200,
                rows: 24,
            },
        )
        .unwrap();
    screen
        .set_client_size(crate::ClientId::new(2), Size { cols: 80, rows: 60 })
        .unwrap();
    screen.recompute_tab_size(TabId::new(0)).expect("TEST");

    assert_eq!(
        screen.tabs.get(&TabId::new(0)).unwrap().size,
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
    new_tab(&mut screen, 1, TabId::new(0));
    screen.add_client(crate::ClientId::new(2)).expect("TEST");

    screen
        .set_client_size(
            crate::ClientId::new(1),
            Size {
                cols: 100,
                rows: 50,
            },
        )
        .unwrap();
    screen
        .set_client_size(
            crate::ClientId::new(2),
            Size {
                cols: 120,
                rows: 40,
            },
        )
        .unwrap();
    screen.resize_to_clients().expect("TEST");

    assert_eq!(
        screen.surface_size(),
        Size {
            cols: 100,
            rows: 40
        },
        "Surface size = (min cols across clients, min rows across clients)"
    );

    screen.remove_client(crate::ClientId::new(2)).expect("TEST");
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
fn attaching_client_records_the_surface_before_a_render_tick() {
    let mut screen = create_new_screen(
        Size {
            cols: 200,
            rows: 60,
        },
        true,
        true,
    );
    new_tab(&mut screen, 1, TabId::new(0));
    screen
        .set_client_size(
            crate::ClientId::new(1),
            Size {
                cols: 200,
                rows: 60,
            },
        )
        .unwrap();
    screen.resize_to_clients().expect("TEST");

    // a second, smaller client attaches - what ScreenInstruction::AddClient runs, minus the
    // debounced render that would otherwise be the first thing to record the new surface
    screen
        .set_client_size(crate::ClientId::new(2), Size { cols: 80, rows: 24 })
        .unwrap();
    screen.add_client(crate::ClientId::new(2)).expect("TEST");
    screen.reflow_all_tabs().expect("TEST");

    // ...and both clients detach before that render tick can fire
    screen.remove_client(crate::ClientId::new(1)).expect("TEST");
    screen.remove_client(crate::ClientId::new(2)).expect("TEST");
    screen.resize_to_clients().expect("TEST");

    assert_eq!(
        screen.surface_size(),
        Size { cols: 80, rows: 24 },
        "the detached session stands in with the surface the attach laid the tabs out over, \
         not the pre-attach one"
    );
    assert_eq!(
        screen.size_for_client(None),
        Size { cols: 80, rows: 24 },
        "a tab created against the detached session (`yinz action new-tab`) is built at that \
         same surface"
    );

    // Rendering with no clients must preserve the last viewer-derived tab geometry.
    screen.render_to_clients().expect("TEST");
    assert_eq!(
        screen.tabs.get(&TabId::new(0)).unwrap().get_display_area(),
        Size { cols: 80, rows: 24 },
        "a render tick against the detached session keeps the attach's layout"
    );
}

#[test]
fn resize_to_clients_reflows_every_tab_and_survives_a_render() {
    let mut screen = create_non_mirrored_screen(Size { cols: 50, rows: 12 });
    new_tab(&mut screen, 1, TabId::new(0));
    screen
        .set_client_size(crate::ClientId::new(1), Size { cols: 50, rows: 12 })
        .unwrap();
    screen.resize_to_clients().expect("TEST");

    screen
        .set_client_size(crate::ClientId::new(1), Size { cols: 40, rows: 10 })
        .unwrap();
    screen.resize_to_clients().expect("TEST");

    let tab = screen.tabs.get(&TabId::new(0)).unwrap();
    assert_eq!(tab.get_display_area(), Size { cols: 40, rows: 10 });

    // Rendering re-derives the layout from this tab's viewers.
    screen.render_to_clients().expect("TEST");
    let tab = screen.tabs.get(&TabId::new(0)).unwrap();
    assert_eq!(tab.get_display_area(), Size { cols: 40, rows: 10 });
}

#[test]
fn character_cell_size_is_derived_from_the_resized_screen() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, true, true);
    new_tab(&mut screen, 1, TabId::new(0));

    // a font-size change: the terminal keeps its pixel size but reports fewer, larger cells
    screen
        .set_client_size(crate::ClientId::new(1), Size { cols: 40, rows: 12 })
        .unwrap();
    screen.resize_to_clients().expect("TEST");
    screen.update_pixel_dimensions(
        crate::ClientId::new(1),
        PixelDimensions {
            text_area_grid_size: None,
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
fn resize_to_clients_sizes_each_tab_from_its_viewers() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_non_mirrored_screen(initial_size);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    screen.add_client(crate::ClientId::new(2)).expect("TEST");
    screen
        .set_client_size(crate::ClientId::new(1), Size { cols: 80, rows: 24 })
        .unwrap();
    screen
        .set_client_size(
            crate::ClientId::new(2),
            Size {
                cols: 160,
                rows: 50,
            },
        )
        .unwrap();
    screen
        .switch_active_tab(TabPosition::new(0), None, true, crate::ClientId::new(2))
        .expect("TEST");
    screen.resize_to_clients().expect("TEST");

    assert_eq!(
        screen.tabs.get(&TabId::new(1)).unwrap().size,
        Size { cols: 80, rows: 24 },
        "The smaller client sizes only its own tab"
    );
    assert_eq!(
        screen.tabs.get(&TabId::new(0)).unwrap().size,
        Size {
            cols: 160,
            rows: 50
        },
        "A tab viewed only by the larger client fits that client"
    );

    screen
        .set_client_size(
            crate::ClientId::new(2),
            Size {
                cols: 240,
                rows: 70,
            },
        )
        .unwrap();
    screen.resize_to_clients().expect("TEST");
    screen.render_to_clients().expect("TEST");

    assert_eq!(
        screen.tabs.get(&TabId::new(0)).unwrap().size,
        Size {
            cols: 240,
            rows: 70
        },
        "The larger client's tab grows even while another tab has a smaller viewer"
    );
    assert_eq!(
        screen.tabs.get(&TabId::new(1)).unwrap().size,
        Size { cols: 80, rows: 24 }
    );

    screen.remove_client(crate::ClientId::new(1)).expect("TEST");
    screen.resize_to_clients().expect("TEST");
    screen.render_to_clients().expect("TEST");

    assert_eq!(
        screen.tabs.get(&TabId::new(0)).unwrap().size,
        Size {
            cols: 240,
            rows: 70
        }
    );
    assert_eq!(
        screen.tabs.get(&TabId::new(1)).unwrap().size,
        Size { cols: 80, rows: 24 },
        "The vacated tab retains its last geometry after detach and render"
    );
}

/// A read-only watcher observes the session without shrinking the tab it follows.
#[test]
fn watcher_size_does_not_constrain_session_bounds() {
    let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 24 });
    new_tab(&mut screen, 1, TabId::new(0));
    screen
        .add_watcher_client(crate::ClientId::new(2))
        .expect("TEST");
    screen.set_watcher_size(crate::ClientId::new(2), Size { cols: 40, rows: 10 });
    screen.resize_to_clients().expect("TEST");

    assert_eq!(
        screen.tabs.get(&TabId::new(0)).unwrap().size,
        Size { cols: 80, rows: 24 },
        "The smaller watcher viewport does not shrink the interactive session"
    );
}

#[test]
fn switching_tabs_recomputes_source_and_destination_from_viewers() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_non_mirrored_screen(initial_size);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    screen.add_client(crate::ClientId::new(2)).expect("TEST");

    screen
        .set_client_size(crate::ClientId::new(1), Size { cols: 80, rows: 24 })
        .unwrap();
    screen
        .set_client_size(
            crate::ClientId::new(2),
            Size {
                cols: 160,
                rows: 50,
            },
        )
        .unwrap();
    screen.recompute_tab_size(TabId::new(1)).expect("TEST");
    assert_eq!(
        screen.tabs.get(&TabId::new(1)).unwrap().size,
        Size { cols: 80, rows: 24 },
        "Both clients on tab 1 → it sizes to the smaller"
    );

    screen
        .switch_active_tab(TabPosition::new(0), None, true, crate::ClientId::new(1))
        .expect("TEST");

    assert_eq!(
        screen.tabs.get(&TabId::new(1)).unwrap().size,
        Size {
            cols: 160,
            rows: 50
        },
        "Source tab grows to fit its remaining larger viewer"
    );
    assert_eq!(
        screen.tabs.get(&TabId::new(0)).unwrap().size,
        Size { cols: 80, rows: 24 },
        "Destination tab fits the arriving smaller viewer"
    );

    screen
        .switch_active_tab(TabPosition::new(1), None, true, crate::ClientId::new(1))
        .expect("TEST");
    screen.render_to_clients().expect("TEST");
    assert_eq!(
        screen.tabs.get(&TabId::new(1)).unwrap().size,
        Size { cols: 80, rows: 24 },
        "Joining the larger viewer shrinks the destination tab"
    );
    assert_eq!(
        screen.tabs.get(&TabId::new(0)).unwrap().size,
        Size { cols: 80, rows: 24 },
        "The now unviewed source tab keeps its last geometry"
    );
}

#[test]
fn unviewed_tab_keeps_its_size_until_visited() {
    let small = Size { cols: 80, rows: 24 };
    let large = Size {
        cols: 160,
        rows: 50,
    };
    let client = ClientId::new(1);
    let mut screen = create_non_mirrored_screen(small);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));

    screen.set_client_size(client, large).unwrap();
    screen.resize_to_clients().expect("TEST");
    screen.render_to_clients().expect("TEST");
    assert_eq!(screen.tabs.get(&TabId::new(0)).unwrap().size, small);
    assert_eq!(screen.tabs.get(&TabId::new(1)).unwrap().size, large);

    screen
        .switch_active_tab(TabPosition::new(0), None, true, client)
        .expect("TEST");
    screen.render_to_clients().expect("TEST");
    assert_eq!(screen.tabs.get(&TabId::new(0)).unwrap().size, large);

    screen.set_client_size(client, small).unwrap();
    screen.resize_to_clients().expect("TEST");
    screen.render_to_clients().expect("TEST");
    assert_eq!(screen.tabs.get(&TabId::new(0)).unwrap().size, small);
    assert_eq!(screen.tabs.get(&TabId::new(1)).unwrap().size, large);
}

#[test]
fn attaching_smaller_client_only_resizes_the_tab_it_joins() {
    let large = Size {
        cols: 160,
        rows: 50,
    };
    let small = Size { cols: 80, rows: 24 };
    let mut screen = create_non_mirrored_screen(large);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    screen
        .register_client_transaction(ClientId::new(2), None, Some(small), true)
        .expect("TEST");
    screen.render_to_clients().expect("TEST");

    assert_eq!(screen.tabs.get(&TabId::new(0)).unwrap().size, large);
    assert_eq!(screen.tabs.get(&TabId::new(1)).unwrap().size, small);
}

/// A large client viewing its own tab, with text past the bounds of a smaller client's tab.
fn large_client_with_margin_text(large: Size, small: Size) -> Screen {
    let mut screen = create_non_mirrored_screen(large);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    screen.add_client(ClientId::new(2)).unwrap();
    screen.set_client_size(ClientId::new(2), small).unwrap();
    screen
        .switch_active_tab(TabPosition::new(0), None, true, ClientId::new(2))
        .unwrap();
    screen
        .tabs
        .get_mut(&TabId::new(1))
        .unwrap()
        .handle_pty_bytes(TerminalId::new(2), b"\x1b[40;120HMARGIN".to_vec())
        .unwrap();
    screen
}

/// The destination paints only its own bounds, so whatever the larger tab left beyond them stays
/// on the host unless the arrival clears it.
#[test]
fn arriving_at_a_tab_pinned_smaller_clears_the_uncovered_margin() {
    let large = Size {
        cols: 160,
        rows: 50,
    };
    let small = Size { cols: 80, rows: 24 };
    let client = ClientId::new(1);
    let mut screen = large_client_with_margin_text(large, small);
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(large);
    render_frame_into(&mut screen, &receiver, client, &mut grid);
    assert!(format!("{grid:?}").contains("MARGIN"));

    screen
        .switch_active_tab(TabPosition::new(0), None, true, client)
        .unwrap();
    render_frame_into(&mut screen, &receiver, client, &mut grid);

    assert_eq!(screen.tabs.get(&TabId::new(0)).unwrap().size, small);
    assert!(
        !format!("{grid:?}").contains("MARGIN"),
        "The source tab's text must not survive beside the smaller destination"
    );
}

/// A watcher repaints whole from the same clear, so it does not keep the source tab's text either.
#[test]
fn a_watcher_following_a_client_to_a_tab_pinned_smaller_loses_the_uncovered_margin() {
    let large = Size {
        cols: 160,
        rows: 50,
    };
    let small = Size { cols: 80, rows: 24 };
    let client = ClientId::new(1);
    let watcher = ClientId::new(3);
    let mut screen = large_client_with_margin_text(large, small);
    screen.set_followed_client(client).unwrap();
    screen.add_watcher_client(watcher).unwrap();
    screen.set_watcher_size(watcher, large);
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(large);
    render_frame_into(&mut screen, &receiver, watcher, &mut grid);
    assert!(format!("{grid:?}").contains("MARGIN"));

    screen
        .switch_active_tab(TabPosition::new(0), None, true, client)
        .unwrap();
    render_frame_into(&mut screen, &receiver, watcher, &mut grid);

    assert!(!format!("{grid:?}").contains("MARGIN"));
}

#[test]
fn closing_a_large_tab_clears_the_margin_of_the_pinned_tab_its_client_lands_on() {
    let large = Size {
        cols: 160,
        rows: 50,
    };
    let small = Size { cols: 80, rows: 24 };
    let client = ClientId::new(1);
    let mut screen = large_client_with_margin_text(large, small);
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(large);
    render_frame_into(&mut screen, &receiver, client, &mut grid);
    assert!(format!("{grid:?}").contains("MARGIN"));

    screen.close_tab_by_id(TabId::new(1)).unwrap();
    render_frame_into(&mut screen, &receiver, client, &mut grid);

    assert_eq!(screen.active_tab_ids.get(&client), Some(&TabId::new(0)));
    assert!(
        !format!("{grid:?}").contains("MARGIN"),
        "The closed tab's text must not survive beside the smaller destination"
    );
}

/// An attach naming its tab belongs there from the start. Passing through another client's tab on
/// the way would lay that tab out at the attaching client's size and back again.
#[test]
fn attaching_with_a_focus_target_leaves_other_tabs_alone() {
    let large = Size {
        cols: 160,
        rows: 50,
    };
    let small = Size { cols: 80, rows: 24 };
    let attaching = ClientId::new(2);
    // The client router sends resizes straight to the screen, so one can overtake the attach.
    for resize_overtook_attach in [false, true] {
        let mut screen = create_non_mirrored_screen(large);
        new_tab(&mut screen, 1, TabId::new(0));
        new_tab(&mut screen, 2, TabId::new(1));
        screen.render_to_clients().expect("TEST");
        assert!(!screen.tabs[&TabId::new(1)].should_clear_display_before_rendering());
        if resize_overtook_attach {
            screen.set_client_size(attaching, small).expect("TEST");
        }

        let mut traces = HashMap::new();
        screen
            .attach_client(
                attaching,
                small,
                Some(TabNumber::new(1)),
                None,
                &mut traces,
                &mut crate::screen::thread_main::PendingEvents::default(),
            )
            .expect("TEST");

        assert_eq!(screen.active_tab_ids.get(&attaching), Some(&TabId::new(0)));
        assert_eq!(screen.tabs[&TabId::new(0)].size, small);
        assert_eq!(screen.tabs[&TabId::new(1)].size, large);
        assert!(
            !screen.tabs[&TabId::new(1)].should_clear_display_before_rendering(),
            "The other client's tab must not be resized by a client that never stays on it"
        );
    }
}

/// The resize was sent after the size the attach carries, so the attach must not undo it.
#[test]
fn a_resize_overtaking_an_attach_outlives_the_size_the_attach_carries() {
    let large = Size {
        cols: 160,
        rows: 50,
    };
    let carried = Size { cols: 80, rows: 24 };
    let current = Size {
        cols: 100,
        rows: 30,
    };
    let attaching = ClientId::new(2);
    for tab_to_focus in [None, Some(TabNumber::new(1))] {
        let mut screen = create_non_mirrored_screen(large);
        new_tab(&mut screen, 1, TabId::new(0));
        screen.set_client_size(attaching, current).expect("TEST");

        screen
            .attach_client(
                attaching,
                carried,
                tab_to_focus,
                None,
                &mut HashMap::new(),
                &mut crate::screen::thread_main::PendingEvents::default(),
            )
            .expect("TEST");

        assert_eq!(screen.client_sizes[&attaching], current);
        assert_eq!(screen.tabs[&TabId::new(0)].size, current);
    }
}

/// The opener's departure frees its remaining viewers before the state report that follows it, not
/// just on the next frame, which reports nothing.
#[test]
fn opening_a_tab_reflows_the_tab_its_opener_left() {
    let large = Size {
        cols: 160,
        rows: 50,
    };
    let small = Size { cols: 80, rows: 24 };
    let mut screen = create_non_mirrored_screen(small);
    new_tab(&mut screen, 1, TabId::new(0));
    screen.add_client(ClientId::new(2)).unwrap();
    screen.set_client_size(ClientId::new(2), large).unwrap();
    screen.resize_to_clients().expect("TEST");
    assert_eq!(screen.tabs[&TabId::new(0)].size, small);

    new_tab(&mut screen, 2, TabId::new(1));

    assert_eq!(screen.active_tab_ids[&ClientId::new(1)], TabId::new(1));
    assert_eq!(screen.tabs[&TabId::new(0)].size, large);
    assert_eq!(screen.tabs[&TabId::new(1)].size, small);
}

#[test]
fn break_pane_to_new_tab_recomputes_source_and_destination() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_non_mirrored_screen(initial_size);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    screen.add_client(crate::ClientId::new(2)).expect("TEST");

    screen
        .set_client_size(crate::ClientId::new(1), Size { cols: 80, rows: 24 })
        .unwrap();
    screen
        .set_client_size(
            crate::ClientId::new(2),
            Size {
                cols: 160,
                rows: 50,
            },
        )
        .unwrap();
    screen
        .switch_active_tab(TabPosition::new(0), None, true, crate::ClientId::new(1))
        .expect("TEST");
    assert_eq!(
        screen.tabs.get(&TabId::new(0)).unwrap().size,
        Size { cols: 80, rows: 24 },
        "Pre-condition: tab 0 sized to client 1"
    );
    assert_eq!(
        screen.tabs.get(&TabId::new(1)).unwrap().size,
        Size {
            cols: 160,
            rows: 50
        },
        "Pre-condition: tab 1 fits its larger viewer"
    );

    {
        let active_tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
        active_tab
            .new_pane(
                PaneId::Terminal(crate::TerminalId::new(99)),
                None,
                None,
                false,
                true,
                NewPanePlacement::default(),
                Some(crate::ClientId::new(1)),
                None,
            )
            .unwrap();
    }

    screen
        .break_pane_to_new_tab(Direction::Right, crate::ClientId::new(1))
        .expect("TEST");

    assert_eq!(
        screen.tabs.get(&TabId::new(1)).unwrap().size,
        Size { cols: 80, rows: 24 },
        "Destination tab shrinks to fit the arriving smaller viewer"
    );
    assert_eq!(
        screen.tabs.get(&TabId::new(0)).unwrap().size,
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
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    screen.add_client(crate::ClientId::new(2)).expect("TEST");

    screen
        .set_client_size(crate::ClientId::new(1), Size { cols: 80, rows: 24 })
        .unwrap();
    screen
        .set_client_size(
            crate::ClientId::new(2),
            Size {
                cols: 160,
                rows: 50,
            },
        )
        .unwrap();
    screen
        .switch_active_tab(TabPosition::new(0), None, true, crate::ClientId::new(2))
        .expect("TEST");
    assert_eq!(
        screen.tabs.get(&TabId::new(0)).unwrap().size,
        Size {
            cols: 160,
            rows: 50
        },
        "Pre-condition: tab 0 fits its larger viewer"
    );
    assert_eq!(
        screen.tabs.get(&TabId::new(1)).unwrap().size,
        Size { cols: 80, rows: 24 },
        "Pre-condition: tab 1 sized to client 1"
    );

    let pane_to_move = PaneId::Terminal(crate::TerminalId::new(42));
    {
        let active_tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
        active_tab
            .new_pane(
                pane_to_move,
                None,
                None,
                false,
                true,
                NewPanePlacement::default(),
                Some(crate::ClientId::new(1)),
                None,
            )
            .unwrap();
    }

    let outcome = screen
        .break_multiple_panes_to_tab_with_id(
            vec![pane_to_move],
            TabId::new(0),
            true,
            crate::ClientId::new(1),
        )
        .expect("TEST");
    assert!(
        outcome.succeeded(),
        "Pre-condition: the pane actually moved"
    );

    assert_eq!(
        screen.tabs.get(&TabId::new(0)).unwrap().size,
        Size { cols: 80, rows: 24 },
        "Destination tab shrinks to min(viewers) once the smaller client arrives"
    );
    assert_eq!(
        screen.tabs.get(&TabId::new(1)).unwrap().size,
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
    new_tab(&mut screen, 1, TabId::new(0));
    screen.add_client(crate::ClientId::new(2)).expect("TEST");

    screen
        .set_client_size(
            crate::ClientId::new(1),
            Size {
                cols: 160,
                rows: 50,
            },
        )
        .unwrap();
    screen
        .set_client_size(crate::ClientId::new(2), Size { cols: 80, rows: 24 })
        .unwrap();
    screen.recompute_tab_size(TabId::new(0)).expect("TEST");
    assert_eq!(
        screen.tabs.get(&TabId::new(0)).unwrap().size,
        Size { cols: 80, rows: 24 },
        "Smaller viewer wins while both clients are on the tab"
    );

    screen.remove_client(crate::ClientId::new(2)).expect("TEST");
    assert_eq!(
        screen.tabs.get(&TabId::new(0)).unwrap().size,
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
    let client_id = ClientId::new(1);
    let small_size = Size { cols: 80, rows: 24 };
    let large_size = Size {
        cols: 160,
        rows: 50,
    };

    screen.set_client_size(client_id, small_size).unwrap();
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    assert_eq!(
        screen.tabs.get(&TabId::new(1)).unwrap().size,
        small_size,
        "Pre-condition: both tabs sized to the small client viewport"
    );

    screen.set_client_size(client_id, large_size).unwrap();
    screen.recompute_tab_size(TabId::new(1)).expect("TEST");
    assert_eq!(
        screen.tabs.get(&TabId::new(1)).unwrap().size,
        large_size,
        "Pre-condition: the active tab follows the client resize"
    );
    assert_eq!(
        screen.tabs.get(&TabId::new(0)).unwrap().size,
        small_size,
        "Pre-condition: the background tab keeps its stale size until it is activated"
    );

    screen.close_tab_by_id(TabId::new(1)).expect("TEST");

    assert_eq!(
        screen.get_active_tab(client_id).unwrap().position,
        TabPosition::new(0),
        "Focus returns to the previous tab"
    );
    assert_eq!(
        screen.tabs.get(&TabId::new(0)).unwrap().size,
        large_size,
        "The tab we return to adopts the current client size"
    );
}

/// A tab created while nothing is attached has no client size to derive one from, so it takes the
/// size the session was last laid out over. Without that memory it is built with no rows, which
/// has no room for a pane and the next render tick closes it.
#[test]
fn a_tab_created_with_no_clients_attached_gets_the_last_known_session_size() {
    let size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    screen.render_to_clients().expect("TEST");
    screen.remove_client(crate::ClientId::new(1)).expect("TEST");

    new_tab(&mut screen, 2, TabId::new(1));

    assert_eq!(
        screen.tabs.get(&TabId::new(1)).map(|tab| tab.size),
        Some(size),
        "A tab created while nothing is attached takes the size the panes were last laid out over"
    );
    screen.render_to_clients().expect("TEST");
    assert!(
        screen.tabs.contains_key(&TabId::new(1)),
        "and it survives the render tick rather than being closed for having no selectable panes"
    );
}

/// A terminal that reports its text area in pixels but not its cell size needs a client size to
/// divide it by. The reply comes off the route thread while the size comes off the
/// server-instruction thread, so it can land first - and it is re-sent only on a font size change,
/// so the size arriving has to be what derives the cell size.
#[test]
fn a_pixel_dimensions_reply_arriving_before_the_client_size_still_derives_the_cell_size() {
    let size = Size { cols: 80, rows: 24 };
    let mut screen = create_non_mirrored_screen(size);
    // the helper seeds a size for client 1; the race is about it not being there yet
    screen.client_sizes.clear();

    screen.update_pixel_dimensions(
        crate::ClientId::new(1),
        PixelDimensions {
            text_area_grid_size: None,
            character_cell_size: None,
            text_area_size: Some(SizeInPixels {
                width: 800,
                height: 480,
            }),
        },
    );
    assert_eq!(
        *screen.character_cell_size.borrow(),
        None,
        "there is no client size to divide the text area by yet"
    );

    screen
        .set_client_size(crate::ClientId::new(1), size)
        .unwrap();

    assert_eq!(
        *screen.character_cell_size.borrow(),
        Some(SizeInPixels {
            width: 10,
            height: 20
        }),
        "the size arriving derives what the earlier reply could not"
    );

    screen
        .set_client_size(
            crate::ClientId::new(1),
            Size {
                cols: 100,
                rows: 30,
            },
        )
        .unwrap();

    assert_eq!(
        *screen.character_cell_size.borrow(),
        Some(SizeInPixels {
            width: 10,
            height: 20
        }),
        "a later resize must not divide the stale text area by the new grid"
    );
}

#[test]
fn failed_destination_sizing_disconnects_without_replaying_pending_events() {
    use crate::screen::thread_main::PendingEvents;
    let large = Size {
        cols: 160,
        rows: 50,
    };
    let small = Size { cols: 80, rows: 24 };
    let tab_id = TabId::new(0);
    let attaching = ClientId::new(2);
    for focus_pane in [false, true] {
        let mut screen = create_non_mirrored_screen(large);
        new_tab(&mut screen, 1, tab_id);
        new_tab(&mut screen, 2, TabId::new(1));
        let receiver = screen_wired_to_server(&mut screen);
        let (plugin, _plugin_receiver) = channels::unbounded();
        let tab = screen.tabs.get_mut(&tab_id).unwrap();
        let saved_senders = tab.senders.clone();
        tab.senders.should_silently_fail = false;
        tab.senders.to_plugin = Some(SenderWithContext::new(plugin));
        tab.senders.to_pty_writer = None;
        let mut traces = HashMap::new();
        let mut pending = PendingEvents::default();
        pending
            .waiting_for_client
            .push(ScreenInstruction::ShowPluginCursor(
                crate::PluginId::new(2),
                attaching,
                Some((2, 2)),
            ));

        screen
            .attach_client(
                attaching,
                small,
                (!focus_pane).then_some(TabNumber::new(1)),
                focus_pane.then_some(PaneId::Terminal(TerminalId::new(1))),
                &mut traces,
                &mut pending,
            )
            .unwrap();

        assert!(traces.is_empty());
        assert_eq!(pending.waiting_for_client.len(), 1);
        assert!(screen.connected_clients.borrow().contains(&attaching));
        assert_eq!(screen.active_tab_ids.get(&attaching), Some(&tab_id));
        assert!(screen.tabs_needing_reflow.contains(&tab_id));
        assert_eq!(
            receiver
                .try_iter()
                .filter(|(instruction, _)| matches!(instruction,
            ServerInstruction::ClientAttachFailed(id) if *id == attaching))
                .count(),
            1
        );

        screen.tabs.get_mut(&tab_id).unwrap().senders = saved_senders;
        screen.remove_client(attaching).unwrap();
        assert!(!screen.connected_clients.borrow().contains(&attaching));
    }
}

/// Switching tabs reflows the destination, which another viewer can keep failing. The attaching
/// client is rejected for that like any other failure to place it, and the session survives.
#[test]
fn failed_switch_to_the_destination_disconnects_the_attaching_client() {
    use crate::screen::thread_main::PendingEvents;
    let large = Size {
        cols: 160,
        rows: 50,
    };
    let small = Size { cols: 80, rows: 24 };
    let client = ClientId::new(1);
    let viewer = ClientId::new(3);
    let failing_tab = TabId::new(1);
    let attaching = ClientId::new(2);
    let mut screen = create_non_mirrored_screen(large);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, failing_tab);
    screen.add_client(viewer).unwrap();
    screen.set_client_size(viewer, large).unwrap();
    // The attaching client registers on this client's tab, and has to leave it for the failing one.
    screen
        .switch_active_tab(screen.tabs[&TabId::new(0)].position, None, true, client)
        .unwrap();
    let receiver = screen_wired_to_server(&mut screen);
    let (plugin, _plugin_receiver) = channels::unbounded();
    let (pty, _pty_receiver) = channels::unbounded();
    let tab = screen.tabs.get_mut(&failing_tab).unwrap();
    let saved_senders = tab.senders.clone();
    tab.senders.should_silently_fail = false;
    tab.senders.to_plugin = Some(SenderWithContext::new(plugin));
    tab.senders.to_pty = Some(SenderWithContext::new(pty));
    tab.senders.to_pty_writer = None;
    screen.tabs_needing_reflow.insert(failing_tab);

    screen
        .attach_client(
            attaching,
            small,
            Some(TabNumber::new(2)),
            None,
            &mut HashMap::new(),
            &mut PendingEvents::default(),
        )
        .unwrap();

    assert_eq!(
        receiver
            .try_iter()
            .filter(|(instruction, _)| matches!(instruction,
            ServerInstruction::ClientAttachFailed(id) if *id == attaching))
            .count(),
        1
    );
    screen.tabs.get_mut(&failing_tab).unwrap().senders = saved_senders;
    screen.remove_client(attaching).unwrap();
}

/// Tabs are sized independently: one that cannot finish must not hold the others back, or abort a
/// frame while it waits for its retry.
#[test]
fn failed_reflow_leaves_other_tabs_and_the_frame_alone() {
    let large = Size {
        cols: 160,
        rows: 50,
    };
    let small = Size { cols: 80, rows: 24 };
    let client = ClientId::new(1);
    let other = ClientId::new(2);
    let failing_tab = TabId::new(0);
    let later_tab = TabId::new(1);
    let mut screen = create_non_mirrored_screen(large);
    new_tab(&mut screen, 1, failing_tab);
    new_tab(&mut screen, 2, later_tab);
    screen.add_client(other).unwrap();
    screen.set_client_size(other, large).unwrap();
    screen
        .switch_active_tab(screen.tabs[&failing_tab].position, None, true, client)
        .unwrap();
    let (pty, _pty_receiver) = channels::unbounded();
    let (plugin, _plugin_receiver) = channels::unbounded();
    let (background, _background_receiver) = channels::unbounded();
    let (server, _server_receiver) = channels::unbounded();
    let tab = screen.tabs.get_mut(&failing_tab).unwrap();
    tab.senders.should_silently_fail = false;
    tab.senders.to_pty_writer = None;
    tab.senders.to_pty = Some(SenderWithContext::new(pty));
    tab.senders.to_plugin = Some(SenderWithContext::new(plugin));
    tab.senders.to_background_jobs = Some(SenderWithContext::new(background));
    tab.senders.to_server = Some(SenderWithContext::new(server));

    screen.set_client_size(client, small).unwrap();
    screen.set_client_size(other, small).unwrap();
    assert!(screen.reflow_all_tabs().is_err());
    assert!(screen.tabs_needing_reflow.contains(&failing_tab));
    assert_eq!(screen.tabs[&later_tab].size, small);
    assert_eq!(screen.last_surface_size, large);

    screen.render_to_clients().unwrap();
    assert!(screen.tabs_needing_reflow.contains(&failing_tab));
}

#[test]
fn incomplete_reflow_retries_at_the_same_display_size() {
    let original = Size {
        cols: 160,
        rows: 50,
    };
    let requested = Size { cols: 80, rows: 24 };
    let client = ClientId::new(1);
    let tab_id = TabId::new(0);
    let mut screen = create_non_mirrored_screen(original);
    new_tab(&mut screen, 1, tab_id);
    let tab = screen.tabs.get_mut(&tab_id).unwrap();
    let saved_senders = tab.senders.clone();
    tab.senders.should_silently_fail = false;
    tab.senders.to_pty_writer = None;
    screen.set_client_size(client, requested).unwrap();
    for _ in 0..2 {
        assert!(screen.reflow_all_tabs().is_err());
        assert_eq!(screen.tabs[&tab_id].get_display_area(), requested);
        assert!(screen.tabs_needing_reflow.contains(&tab_id));
        assert_eq!(screen.last_surface_size, original);
    }
    // No viewers must preserve the incomplete reflow for the next visit.
    screen.client_sizes.remove(&client);
    screen.reflow_all_tabs().unwrap();
    assert!(screen.tabs_needing_reflow.contains(&tab_id));
    screen.set_client_size(client, requested).unwrap();
    screen.tabs.get_mut(&tab_id).unwrap().senders = saved_senders;
    screen.reflow_all_tabs().unwrap();
    assert!(!screen.tabs_needing_reflow.contains(&tab_id));
    assert_eq!(screen.last_surface_size, requested);
    assert_tab_geometry(&screen, tab_id, requested);

    screen
        .tabs
        .get_mut(&tab_id)
        .unwrap()
        .senders
        .should_silently_fail = false;
    screen.set_client_size(client, original).unwrap();
    assert!(screen.reflow_all_tabs().is_err());
    screen.close_tab_by_id(tab_id).unwrap();
    assert!(!screen.tabs_needing_reflow.contains(&tab_id));
}

/// Keeps a tab's PTY resizes failing while everything else it sends still gets through.
fn fail_pty_resizes(screen: &mut Screen, tab_id: TabId) -> impl Sized + use<> {
    let (pty, pty_receiver) = channels::unbounded();
    let (plugin, plugin_receiver) = channels::unbounded();
    let (background, background_receiver) = channels::unbounded();
    let (server, server_receiver) = channels::unbounded();
    let tab = screen.tabs.get_mut(&tab_id).unwrap();
    tab.senders.should_silently_fail = false;
    tab.senders.to_pty_writer = None;
    tab.senders.to_pty = Some(SenderWithContext::new(pty));
    tab.senders.to_plugin = Some(SenderWithContext::new(plugin));
    tab.senders.to_background_jobs = Some(SenderWithContext::new(background));
    tab.senders.to_server = Some(SenderWithContext::new(server));
    (
        pty_receiver,
        plugin_receiver,
        background_receiver,
        server_receiver,
    )
}

/// The switch has already moved the client, so the tab it left failing to grow for its remaining
/// viewer must neither undo the arrival's sizing nor reach the screen thread as a fatal error.
#[test]
fn a_tab_failing_to_reflow_does_not_abandon_a_switch_away_from_it() {
    let large = Size {
        cols: 160,
        rows: 50,
    };
    let small = Size { cols: 80, rows: 24 };
    let leaving = ClientId::new(1);
    let source = TabId::new(1);
    let destination = TabId::new(0);
    let mut screen = create_non_mirrored_screen(small);
    new_tab(&mut screen, 1, destination);
    new_tab(&mut screen, 2, source);
    // Last laid out for a larger viewer, so arriving has to shrink it.
    screen
        .tabs
        .get_mut(&destination)
        .unwrap()
        .resize_whole_tab(large)
        .unwrap();
    screen.add_client(ClientId::new(2)).unwrap();
    screen.set_client_size(ClientId::new(2), large).unwrap();
    screen.resize_to_clients().expect("TEST");
    let _receivers = fail_pty_resizes(&mut screen, source);

    screen
        .switch_active_tab(screen.tabs[&destination].position, None, true, leaving)
        .expect("a failed reflow retries on the next frame");

    assert_eq!(screen.active_tab_ids[&leaving], destination);
    assert_eq!(screen.tabs[&destination].size, small);
    assert!(screen.tabs_needing_reflow.contains(&source));
}

/// Reports the failure to the caller, but still schedules the frame that retries it.
#[test]
fn an_incomplete_reflow_still_schedules_the_frame_that_retries_it() {
    let tab_id = TabId::new(0);
    let mut screen = create_non_mirrored_screen(Size {
        cols: 160,
        rows: 50,
    });
    new_tab(&mut screen, 1, tab_id);
    let (background, background_receiver) = channels::unbounded();
    screen.bus.senders.to_background_jobs = Some(SenderWithContext::new(background));
    let _receivers = fail_pty_resizes(&mut screen, tab_id);
    screen
        .set_client_size(ClientId::new(1), Size { cols: 80, rows: 24 })
        .unwrap();

    assert!(screen.resize_to_clients().is_err());

    assert!(background_receiver
        .try_iter()
        .any(|(job, _)| matches!(job, BackgroundJob::RenderToClients)));
}

fn assert_tab_geometry(screen: &Screen, tab_id: TabId, size: Size) {
    let tab = &screen.tabs[&tab_id];
    assert_eq!(tab.size, size);
    assert_eq!(tab.get_display_area(), size);
    let geom = tab.get_tiled_panes().next().unwrap().1.position_and_size();
    assert_eq!(geom.cols.as_usize(), size.cols);
    assert_eq!(geom.rows.as_usize(), size.rows);
}

/// A screen whose only tab has a floating pane, with client 2 attaching at `small` after the
/// writer's receiver has gone. Floating panes clone their senders when the tab is built, so this
/// is the one way to fail their PTY resizes. Begin `Shutdown` to recover them through the writer's
/// normal shutdown tolerance, without replacing the tab.
fn screen_attaching_to_failing_floating_pty(
    large: Size,
    small: Size,
    tab_id: TabId,
    before_attach: impl FnOnce(&mut Screen),
) -> (Screen, Shutdown, impl Sized) {
    let attaching = ClientId::new(2);
    let mut screen = create_non_mirrored_screen(large);
    let saved_senders = screen.bus.senders.clone();
    let shutdown = Shutdown::default();
    let (writer, writer_receiver) = channels::unbounded();
    let (plugin, plugin_receiver) = channels::unbounded();
    let (pty, pty_receiver) = channels::unbounded();
    let (background, background_receiver) = channels::unbounded();
    let (server, server_receiver) = channels::unbounded();
    screen.bus.senders.to_pty_writer = Some(shutdown.sender(writer));
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(plugin));
    screen.bus.senders.to_pty = Some(SenderWithContext::new(pty));
    screen.bus.senders.to_background_jobs = Some(SenderWithContext::new(background));
    screen.bus.senders.to_server = Some(SenderWithContext::new(server));
    screen.bus.senders.should_silently_fail = false;
    new_tab(&mut screen, 1, tab_id);
    add_pane_to_tab(
        &mut screen,
        tab_id,
        PaneId::Terminal(TerminalId::new(3)),
        true,
    );
    before_attach(&mut screen);
    screen.bus.senders = saved_senders;
    screen_wired_to_server(&mut screen);
    drop(writer_receiver);
    let mut traces = HashMap::new();
    screen
        .attach_client(
            attaching,
            small,
            Some(TabNumber::new(1)),
            None,
            &mut traces,
            &mut crate::screen::thread_main::PendingEvents::default(),
        )
        .unwrap();
    let receivers = (
        plugin_receiver,
        pty_receiver,
        background_receiver,
        server_receiver,
    );
    (screen, shutdown, receivers)
}

#[test]
fn removal_retries_reflow_after_floating_pty_failure() {
    let large = Size {
        cols: 160,
        rows: 50,
    };
    let small = Size { cols: 80, rows: 24 };
    let tab_id = TabId::new(0);
    let (mut screen, shutdown, _receivers) =
        screen_attaching_to_failing_floating_pty(large, small, tab_id, |_| {});
    assert_eq!(screen.tabs[&tab_id].size, small);
    assert_eq!(screen.tabs[&tab_id].get_display_area(), small);
    assert!(screen.tabs_needing_reflow.contains(&tab_id));
    shutdown.begin();
    screen.remove_client(ClientId::new(2)).unwrap();
    screen.reflow_all_tabs().unwrap();
    assert_eq!(screen.client_sizes.get(&ClientId::new(1)), Some(&large));
    assert_tab_geometry(&screen, tab_id, large);
    assert!(!screen.tabs_needing_reflow.contains(&tab_id));
}

#[test]
fn floating_pty_failure_keeps_fullscreen_through_the_retry() {
    let large = Size {
        cols: 160,
        rows: 50,
    };
    let small = Size { cols: 80, rows: 24 };
    let tab_id = TabId::new(0);
    let pane_id = PaneId::Terminal(TerminalId::new(3));
    let (mut screen, shutdown, _receivers) =
        screen_attaching_to_failing_floating_pty(large, small, tab_id, |screen| {
            let tab = screen.tabs.get_mut(&tab_id).unwrap();
            tab.toggle_pane_fullscreen(pane_id);
            assert_eq!(tab.fullscreen_pane_id(), Some(pane_id));
        });
    let assert_fullscreen_at_small = |screen: &Screen| {
        let tab = &screen.tabs[&tab_id];
        assert_eq!(tab.fullscreen_pane_id(), Some(pane_id));
        let geom = tab.get_pane_with_id(pane_id).unwrap().current_geom();
        assert_eq!(
            (geom.cols.as_usize(), geom.rows.as_usize()),
            (small.cols, small.rows),
        );
    };
    assert!(screen.tabs_needing_reflow.contains(&tab_id));
    assert_fullscreen_at_small(&screen);
    shutdown.begin();
    screen.reflow_all_tabs().unwrap();
    assert!(!screen.tabs_needing_reflow.contains(&tab_id));
    assert_fullscreen_at_small(&screen);
}

/// Capture each delivered stream, including the separate watcher pass.
fn client_frame_bytes(screen: &mut Screen, receiver: &ServerReceiver) -> HashMap<ClientId, String> {
    screen.render_to_clients().unwrap();
    let mut bytes = HashMap::<ClientId, String>::new();
    for (instruction, _) in receiver.try_iter() {
        if let ServerInstruction::Render(Some(clients)) = instruction {
            for (client, stream) in clients {
                bytes.entry(client).or_default().push_str(&stream);
            }
        }
    }
    bytes
}

fn add_arrival_test_layers(screen: &mut Screen, tab_id: TabId) {
    let (plugin, _receiver) = channels::unbounded();
    let tab = screen.tabs.get_mut(&tab_id).unwrap();
    tab.senders.to_plugin = Some(SenderWithContext::new(plugin));
    for (pane, placement) in [
        (
            PaneId::Plugin(crate::PluginId::new(4)),
            NewPanePlacement::default(),
        ),
        (
            PaneId::Terminal(TerminalId::new(5)),
            NewPanePlacement::Floating(Some(FloatingPaneCoordinates {
                x: Some(PercentOrFixed::Fixed(4)),
                y: Some(PercentOrFixed::Fixed(0)),
                width: Some(PercentOrFixed::Fixed(28)),
                height: Some(PercentOrFixed::Fixed(6)),
                ..Default::default()
            })),
        ),
        (
            PaneId::Plugin(crate::PluginId::new(6)),
            NewPanePlacement::Overlay(OverlayPlacement::dismissible(Some(
                FloatingPaneCoordinates {
                    x: Some(PercentOrFixed::Fixed(44)),
                    y: Some(PercentOrFixed::Fixed(2)),
                    width: Some(PercentOrFixed::Fixed(28)),
                    height: Some(PercentOrFixed::Fixed(6)),
                    ..Default::default()
                },
            ))),
        ),
    ] {
        tab.new_pane(
            pane,
            None,
            None,
            false,
            true,
            placement,
            Some(ClientId::new(2)),
            None,
        )
        .unwrap();
    }
    tab.show_floating_panes();
    tab.handle_pty_bytes(
        TerminalId::new(5),
        b"\x1b[3;1HFLOATING-CONTENT\x1b[1;1H".to_vec(),
    )
    .unwrap();
    for client in [ClientId::new(1), ClientId::new(2), ClientId::new(7)] {
        for plugin in [crate::PluginId::new(4), crate::PluginId::new(6)] {
            tab.handle_plugin_bytes(
                plugin,
                client,
                format!("\x1b[3;1HPLUGIN-{plugin}-{client}\x1b[1;1H").into_bytes(),
            )
            .unwrap();
        }
    }
}

#[test]
fn arrival_repaints_only_arriving_clients_and_their_watchers() {
    let large = Size {
        cols: 160,
        rows: 50,
    };
    let small = Size { cols: 80, rows: 24 };
    let arriving = ClientId::new(1);
    let viewer = ClientId::new(2);
    let watcher = ClientId::new(3);
    let second_arrival = ClientId::new(7);
    let destination = TabId::new(0);
    for frame_style in [PaneFrameStyle::Full, PaneFrameStyle::None] {
        for layered in [false, true] {
            for followed in [arriving, viewer] {
                let mut screen = large_client_with_margin_text(large, small);
                screen.add_client(second_arrival).unwrap();
                screen.set_client_size(second_arrival, large).unwrap();
                if layered {
                    add_arrival_test_layers(&mut screen, destination);
                }
                let tab = screen.tabs.get_mut(&destination).unwrap();
                tab.set_pane_frames(frame_style);
                tab.handle_pty_bytes(
                    TerminalId::new(1),
                    b"\x1b[10;1HDESTINATION-CONTENT".to_vec(),
                )
                .unwrap();
                screen.set_followed_client(followed).unwrap();
                screen.add_watcher_client(watcher).unwrap();
                screen.set_watcher_size(watcher, large);
                let receiver = screen_wired_to_server(&mut screen);
                let mut grids: HashMap<_, _> = [arriving, viewer, watcher, second_arrival]
                    .into_iter()
                    .map(|client| (client, grid_for_render_snapshot(large)))
                    .collect();
                for (client, bytes) in client_frame_bytes(&mut screen, &receiver) {
                    take_snapshot_and_cursor_coordinates(&bytes, grids.get_mut(&client).unwrap());
                }
                assert!(format!("{:?}", grids[&arriving]).contains("MARGIN"));
                assert!(format!("{:?}", grids[&viewer]).contains("DESTINATION-CONTENT"));
                for client in [arriving, second_arrival] {
                    screen
                        .switch_active_tab(TabPosition::new(0), None, true, client)
                        .unwrap();
                }
                // Both arrivals queue before the same controlled frame. The destination stays
                // pinned, so neither arrival is allowed to force a geometry repaint.
                assert_eq!(screen.tabs[&destination].size, small);
                let streams = client_frame_bytes(&mut screen, &receiver);
                for client in [arriving, second_arrival] {
                    let bytes = &streams[&client];
                    assert!(bytes.contains("\x1b[2J"), "{client}: {bytes:?}");
                    assert!(bytes.contains("DESTINATION-CONTENT"), "{client}: {bytes:?}");
                    if layered {
                        assert!(bytes.contains("FLOATING-CONTENT"), "{bytes:?}");
                        assert!(bytes.contains(&format!("PLUGIN-6-{client}")), "{bytes:?}");
                        assert!(bytes.contains(&format!("PLUGIN-4-{client}")), "{bytes:?}");
                    }
                }
                for client in [viewer, watcher] {
                    let bytes = streams.get(&client).map(String::as_str).unwrap_or_default();
                    let repainted = client == watcher && followed == arriving;
                    assert_eq!(bytes.contains("\x1b[2J"), repainted, "{client}: {bytes:?}");
                    assert_eq!(
                        bytes.contains("DESTINATION-CONTENT"),
                        repainted,
                        "{client}: {bytes:?}"
                    );
                    if layered && !repainted {
                        assert!(!bytes.contains("FLOATING-CONTENT"), "style={frame_style:?} layered={layered} followed={followed} client={client}: {bytes:?}");
                        assert!(!bytes.contains(&format!("PLUGIN-6-{viewer}")), "{bytes:?}");
                    }
                }
                for (client, bytes) in streams {
                    take_snapshot_and_cursor_coordinates(&bytes, grids.get_mut(&client).unwrap());
                }
                for client in [arriving, second_arrival] {
                    let snapshot = format!("{:?}", grids[&client]);
                    assert!(!snapshot.contains("MARGIN"), "{snapshot}");
                    assert!(snapshot.contains("DESTINATION-CONTENT"), "{snapshot}");
                    if layered {
                        // The floating pane covers the top chrome, which must not overwrite it.
                        assert!(snapshot.contains("FLOATING-CONTENT"), "{snapshot}");
                        assert!(
                            snapshot.contains(&format!("PLUGIN-6-{client}")),
                            "{snapshot}"
                        );
                    }
                }
                // A full repaint must not leave another client's plugin grids, frames, or
                // boundary caches invalidated for the following unchanged frame.
                for bytes in client_frame_bytes(&mut screen, &receiver).values() {
                    assert!(!bytes.contains("\x1b[2J"), "{bytes:?}");
                    assert!(!bytes.contains("DESTINATION-CONTENT"), "{bytes:?}");
                    assert!(!bytes.contains("FLOATING-CONTENT"), "{bytes:?}");
                    assert!(!bytes.contains("PLUGIN-"), "{bytes:?}");
                    assert!(!bytes.contains('┌') && !bytes.contains('└'), "{bytes:?}");
                }
            }
        }
    }
}

#[test]
fn departure_cancels_an_arrival_repaint_before_the_frame() {
    let large = Size {
        cols: 160,
        rows: 50,
    };
    let small = Size { cols: 80, rows: 24 };
    let mut screen = large_client_with_margin_text(large, small);
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .handle_pty_bytes(TerminalId::new(1), b"\x1b[10;1HSTABLE-CONTENT".to_vec())
        .unwrap();
    let receiver = screen_wired_to_server(&mut screen);
    client_frame_bytes(&mut screen, &receiver);
    screen
        .switch_active_tab(TabPosition::new(0), None, true, ClientId::new(1))
        .unwrap();
    screen
        .switch_active_tab(TabPosition::new(1), None, true, ClientId::new(1))
        .unwrap();
    for _ in 0..2 {
        let streams = client_frame_bytes(&mut screen, &receiver);
        let bytes = streams
            .get(&ClientId::new(2))
            .map(String::as_str)
            .unwrap_or_default();
        assert!(!bytes.contains("\x1b[2J"), "{bytes:?}");
        assert!(!bytes.contains("STABLE-CONTENT"), "{bytes:?}");
    }
}

#[test]
fn a_geometry_clear_satisfies_the_arrival_repaint() {
    let large = Size {
        cols: 160,
        rows: 50,
    };
    let small = Size { cols: 80, rows: 24 };
    let mut screen = large_client_with_margin_text(large, small);
    let receiver = screen_wired_to_server(&mut screen);
    client_frame_bytes(&mut screen, &receiver);
    screen
        .switch_active_tab(TabPosition::new(0), None, true, ClientId::new(1))
        .unwrap();
    screen.set_client_size(ClientId::new(2), large).unwrap();
    screen.resize_to_clients().unwrap();
    let streams = client_frame_bytes(&mut screen, &receiver);
    for client in [ClientId::new(1), ClientId::new(2)] {
        assert_eq!(
            streams[&client].matches("\x1b[2J").count(),
            1,
            "{:?}",
            streams[&client]
        );
    }
    for bytes in client_frame_bytes(&mut screen, &receiver).values() {
        assert!(!bytes.contains("\x1b[2J"), "{bytes:?}");
    }
}

#[test]
fn post_layout_resize_failure_keeps_the_committed_layout_and_retries_at_the_same_size() {
    let size = Size { cols: 80, rows: 24 };
    let tab_id = TabId::new(1);
    let mut screen = create_non_mirrored_screen(size);
    let (writer, writer_receiver) = channels::unbounded();
    screen.bus.senders.to_pty_writer = Some(SenderWithContext::new(writer));
    new_tab(&mut screen, 1, TabId::new(0));
    screen
        .new_tab(tab_id, (vec![], vec![]), None, Some(ClientId::new(1)))
        .unwrap();
    let saved_senders = screen.tabs[&tab_id].senders.clone();
    let _receivers = fail_pty_resizes(&mut screen, tab_id);
    screen
        .apply_layout(
            TiledPaneLayout::default(),
            vec![],
            vec![(TerminalId::new(9), None)],
            vec![],
            HashMap::new(),
            tab_id,
            true,
            ClientId::new(1),
            None,
        )
        .expect("a committed layout must not be rejected by its post-layout resize");
    assert!(!screen.tabs[&tab_id].is_pending());
    assert!(screen.tabs[&tab_id]
        .get_pane_with_id(PaneId::Terminal(TerminalId::new(9)))
        .is_some());
    assert_tab_geometry(&screen, tab_id, size);
    for _ in 0..2 {
        assert!(screen.tabs_needing_reflow.contains(&tab_id));
        assert!(screen.recompute_tab_size(tab_id).is_err());
        assert_tab_geometry(&screen, tab_id, size);
    }
    writer_receiver.try_iter().for_each(drop);
    screen.tabs.get_mut(&tab_id).unwrap().senders = saved_senders;
    screen.recompute_tab_size(tab_id).unwrap();
    assert!(!screen.tabs_needing_reflow.contains(&tab_id));
    assert_tab_geometry(&screen, tab_id, size);
    assert!(writer_receiver.try_iter().any(|(instruction, _)| matches!(instruction,
        PtyWriteInstruction::ResizePty(id, cols, rows, _, _) if id == TerminalId::new(9) && cols > 0 && rows > 0
    )));
}

#[test]
fn recompute_tab_size_instruction_survives_resize_failure_and_schedules_recovery() {
    let large = Size {
        cols: 160,
        rows: 50,
    };
    let small = Size { cols: 80, rows: 24 };
    let client = ClientId::new(1);
    let terminal = TerminalId::new(0);
    let mut mock = MockScreen::new(large);
    let writer_shutdown = Shutdown::default();
    let (writer, writer_receiver) = channels::unbounded();
    let writer = writer_shutdown.sender(writer);
    let (background, background_receiver) = channels::unbounded();
    let background = SenderWithContext::new(background);
    // Start the real instruction loop directly: no sleeps or simulated dispatch. The writer
    // remains connected until the initial layout and content are visibly committed.
    let mut bus = Bus::new(
        vec![mock.screen_receiver.take().unwrap()],
        Some(&mock.to_screen),
        Some(&mock.to_pty),
        Some(&mock.to_plugin),
        Some(&mock.to_server),
        Some(&writer),
        Some(&background),
        Some(Box::new(mock.os_input.clone())),
    );
    bus.senders.session = mock.session_metadata.senders.session.clone();
    let attributes = mock.client_attributes.clone();
    let mut config = mock.config.clone();
    config.options.agent_pane_min_columns = Some(0);
    config.options.agent_pane_min_rows = Some(0);
    let thread = std::thread::spawn(move || {
        screen_thread_main(
            bus,
            None,
            attributes,
            config,
            false,
            None,
            None,
            None,
            Box::default(),
        )
        .expect("the screen thread must survive a failed resize");
    });
    let events = Watched::new(vec![]);
    let server_receiver = mock.server_receiver.take().unwrap();
    let server_thread =
        log_actions_in_thread!(events, ServerInstruction::KillSession, server_receiver);
    mock.to_screen
        .send(ScreenInstruction::NewTab(
            None,
            None,
            Some(TiledPaneLayout::default()),
            vec![],
            None,
            (Some(vec![]), Some(vec![])),
            None,
            false,
            true,
            client,
            None,
        ))
        .unwrap();
    mock.to_screen
        .send(ScreenInstruction::ApplyLayout(
            TiledPaneLayout::default(),
            vec![],
            vec![(terminal, None)],
            vec![],
            vec![],
            HashMap::new(),
            TabId::new(0),
            true,
            client,
            None,
            None,
        ))
        .unwrap();
    mock.to_screen
        .send(ScreenInstruction::PtyBytes(terminal, b"READY".to_vec()))
        .unwrap();
    mock.to_screen
        .send(ScreenInstruction::RenderToClients)
        .unwrap();
    wait_for_settled_render(&events, large, "initial layout", |frame| {
        frame.contains("READY")
    });
    drop(writer_receiver);
    background_receiver.try_iter().for_each(drop);
    mock.to_screen
        .send(ScreenInstruction::RecomputeTabSize(client, small))
        .unwrap();
    // This reply is a barrier: the resize handler has returned to the receive loop, and
    // only its scheduling can have queued a retry since the initialization jobs were drained.
    let (response, receiver) = crossbeam::channel::unbounded();
    mock.to_screen
        .send(ScreenInstruction::GetPaneInfo {
            pane_id: PaneId::Terminal(terminal),
            response_channel: response,
        })
        .unwrap();
    assert!(receiver
        .recv_timeout(yinz_test_support::default_timeout())
        .unwrap()
        .is_some());
    assert!(background_receiver
        .try_iter()
        .any(|(job, _)| matches!(job, BackgroundJob::RenderToClients)));
    mock.to_screen
        .send(ScreenInstruction::PtyBytes(
            terminal,
            b"\x1b[10;1HALIVE-AFTER-FAILURE".to_vec(),
        ))
        .unwrap();
    mock.to_screen
        .send(ScreenInstruction::RenderToClients)
        .unwrap();
    wait_for_settled_render(
        &events,
        small,
        "instructions after resize failure",
        |frame| frame.contains("ALIVE-AFTER-FAILURE"),
    );

    // Recover the same sender using its normal shutdown tolerance, as the floating-resize
    // regressions do. The separate layout test verifies actual size resends and retry removal.
    writer_shutdown.begin();
    mock.to_screen
        .send(ScreenInstruction::PtyBytes(
            terminal,
            b"\x1b[20;60HRECOVERED".to_vec(),
        ))
        .unwrap();
    mock.to_screen
        .send(ScreenInstruction::RenderToClients)
        .unwrap();
    wait_for_settled_render(
        &events,
        small,
        "render after resize dependency recovery",
        |frame| frame.contains("ALIVE-AFTER-FAILURE") && frame.contains("RECOVERED"),
    );
    let (response, receiver) = crossbeam::channel::unbounded();
    mock.to_screen
        .send(ScreenInstruction::GetPaneInfo {
            pane_id: PaneId::Terminal(terminal),
            response_channel: response,
        })
        .unwrap();
    let pane = receiver
        .recv_timeout(yinz_test_support::default_timeout())
        .unwrap()
        .unwrap();
    assert!(pane.pane_x.get() + pane.pane_columns.get() <= small.cols);
    assert!(pane.pane_y.get() + pane.pane_rows.get() <= small.rows);
    mock.teardown(vec![thread, server_thread]);
}

#[test]
fn arrival_preserves_incremental_updates_for_an_existing_viewer_and_its_watcher() {
    let large = Size {
        cols: 160,
        rows: 50,
    };
    let small = Size { cols: 80, rows: 24 };
    let viewer = ClientId::new(2);
    let watcher = ClientId::new(3);
    let mut screen = large_client_with_margin_text(large, small);
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .handle_pty_bytes(
            TerminalId::new(1),
            b"\x1b[15;1HUNCHANGED-CONTENT\x1b[1;1H".to_vec(),
        )
        .unwrap();
    screen.set_followed_client(viewer).unwrap();
    screen.add_watcher_client(watcher).unwrap();
    screen.set_watcher_size(watcher, small);
    let receiver = screen_wired_to_server(&mut screen);
    client_frame_bytes(&mut screen, &receiver);
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .handle_pty_bytes(
            TerminalId::new(1),
            b"\x1b[3;1HINCREMENTAL-UPDATE\x1b[1;1H".to_vec(),
        )
        .unwrap();
    screen
        .switch_active_tab(TabPosition::new(0), None, true, ClientId::new(1))
        .unwrap();
    let streams = client_frame_bytes(&mut screen, &receiver);
    assert!(streams[&ClientId::new(1)].contains("UNCHANGED-CONTENT"));
    for client in [viewer, watcher] {
        let bytes = &streams[&client];
        assert!(bytes.contains("INCREMENTAL-UPDATE"), "{client}: {bytes:?}");
        assert!(!bytes.contains("UNCHANGED-CONTENT"), "{client}: {bytes:?}");
        assert!(!bytes.contains("\x1b[2J"), "{client}: {bytes:?}");
    }
}

/// A collapsed stack member has no contents, only a frame the followed client has already cached,
/// so a watcher's whole repaint, which clears its display, must not skip it.
#[test]
fn a_watcher_repainted_whole_gets_collapsed_stack_titles() {
    let size = Size {
        cols: 160,
        rows: 50,
    };
    let followed = ClientId::new(2);
    let watcher = ClientId::new(3);
    let mut screen = large_client_with_margin_text(size, size);
    let tab = screen.tabs.get_mut(&TabId::new(0)).unwrap();
    tab.new_pane(
        PaneId::Terminal(TerminalId::new(3)),
        None,
        None,
        false,
        false,
        NewPanePlacement::Stacked {
            pane_id_to_stack_under: Some(PaneId::Terminal(TerminalId::new(1))),
            borderless: None,
        },
        Some(followed),
        None,
    )
    .unwrap();
    let collapsed = [TerminalId::new(1), TerminalId::new(3)]
        .into_iter()
        .find(|id| {
            let geom = tab
                .get_pane_with_id(PaneId::Terminal(*id))
                .unwrap()
                .current_geom();
            geom.is_stacked() && geom.rows.is_fixed()
        })
        .expect("one stack member is collapsed");
    tab.handle_pty_bytes(collapsed, b"\x1b]0;COLLAPSED-TITLE\x07".to_vec())
        .unwrap();
    let receiver = screen_wired_to_server(&mut screen);
    screen.set_followed_client(followed).unwrap();
    screen.add_watcher_client(watcher).unwrap();
    screen.set_watcher_size(watcher, size);
    client_frame_bytes(&mut screen, &receiver);
    screen
        .watcher_clients
        .get_mut(&watcher)
        .unwrap()
        .set_force_render();

    let streams = client_frame_bytes(&mut screen, &receiver);

    assert!(
        streams[&watcher].contains("COLLAPSED-TITLE"),
        "{:?}",
        streams[&watcher]
    );
}

/// Another viewer's tab failing to reflow says nothing about whether this client can be placed.
#[test]
fn attaching_survives_a_reflow_failure_in_a_tab_it_is_not_joining() {
    use crate::screen::thread_main::PendingEvents;
    let large = Size {
        cols: 160,
        rows: 50,
    };
    let small = Size { cols: 80, rows: 24 };
    let attaching = ClientId::new(3);
    let failing_tab = TabId::new(0);
    let destination = TabId::new(1);
    let mut screen = large_client_with_margin_text(large, small);
    let receiver = screen_wired_to_server(&mut screen);
    let _receivers = fail_pty_resizes(&mut screen, failing_tab);
    screen
        .set_client_size(ClientId::new(2), Size { cols: 70, rows: 20 })
        .unwrap();
    assert!(screen.reflow_all_tabs().is_err());
    let mut traces = HashMap::new();

    screen
        .attach_client(
            attaching,
            large,
            Some(TabNumber::new(2)),
            None,
            &mut traces,
            &mut PendingEvents::default(),
        )
        .unwrap();

    assert_eq!(screen.active_tab_ids.get(&attaching), Some(&destination));
    assert!(
        !receiver.try_iter().any(|(instruction, _)| matches!(
            instruction,
            ServerInstruction::ClientAttachFailed(id) if id == attaching
        )),
        "The attaching client's own tab reflowed cleanly"
    );
}

/// The client has already been re-homed, so a failed reflow retries on the frame this schedules.
#[test]
fn closing_a_tab_survives_a_reflow_failure_in_the_tab_its_client_lands_on() {
    let large = Size {
        cols: 160,
        rows: 50,
    };
    let small = Size { cols: 80, rows: 24 };
    let surviving_tab = TabId::new(0);
    let mut screen = large_client_with_margin_text(large, small);
    let _receivers = fail_pty_resizes(&mut screen, surviving_tab);
    screen
        .set_client_size(ClientId::new(2), Size { cols: 70, rows: 20 })
        .unwrap();

    screen.close_tab_by_id(TabId::new(1)).unwrap();

    assert_eq!(
        screen.active_tab_ids.get(&ClientId::new(1)),
        Some(&surviving_tab)
    );
    assert!(screen.tabs_needing_reflow.contains(&surviving_tab));
}

/// A clear alone does not repaint tiled panes, so it cannot stand in for an arrival's repaint.
#[test]
fn a_clear_without_a_forced_repaint_keeps_the_arrival_repaint() {
    let large = Size {
        cols: 160,
        rows: 50,
    };
    let small = Size { cols: 80, rows: 24 };
    let client = ClientId::new(1);
    let mut screen = large_client_with_margin_text(large, small);
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .handle_pty_bytes(
            TerminalId::new(1),
            b"\x1b[15;1HUNCHANGED-CONTENT\x1b[1;1H".to_vec(),
        )
        .unwrap();
    let receiver = screen_wired_to_server(&mut screen);
    client_frame_bytes(&mut screen, &receiver);
    screen
        .switch_active_tab(TabPosition::new(0), None, true, client)
        .unwrap();
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .set_should_clear_display_before_rendering();

    let streams = client_frame_bytes(&mut screen, &receiver);

    assert!(
        streams[&client].contains("UNCHANGED-CONTENT"),
        "{:?}",
        streams[&client]
    );
    assert_eq!(streams[&client].matches("\x1b[2J").count(), 1);
}

#[test]
fn smaller_client_centers_entire_view_and_detach_restores_origin() {
    let large = Size {
        cols: 101,
        rows: 31,
    };
    let mut screen = create_new_screen(large, true, true);
    let client = ClientId::new(1);
    let other = ClientId::new(2);
    new_tab(&mut screen, 1, TabId::new(0));
    screen.add_client(other).expect("TEST");
    screen
        .set_client_size(other, Size { cols: 80, rows: 24 })
        .expect("TEST");
    screen.resize_to_clients().expect("TEST");
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(large);
    let (cursor, _) = render_frame_and_cursor_into(&mut screen, &receiver, client, &mut grid);
    let logical_cursor = pane_cursor(&mut screen, client);
    assert_eq!(cursor, Some((logical_cursor.0 + 10, logical_cursor.1 + 3)));
    assert_eq!(screen.client_presentations[&client].1.offset(), (10, 3));
    assert_eq!(screen.client_presentations[&other].1.offset(), (0, 0));
    assert_eq!(
        screen.client_presentations[&client].1.background,
        screen.style.pane_surround()
    );

    let cells = grid.as_character_lines();
    for (x, y) in [(0, 0), (100, 30), (0, 15), (100, 15)] {
        assert_eq!(
            cells[y][x].styles.background,
            Some(screen.style.pane_surround().into())
        );
    }
    let (to_plugin, plugin_receiver) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(to_plugin));
    screen.handle_mouse_event(
        MouseEvent::new_right_press_event(Position::new(0, 0)),
        client,
    );
    assert!(!plugin_receiver.try_iter().any(|(instruction, _)| matches!(
        instruction,
        PluginInstruction::RequestContextMenu { .. }
    )));
    screen.handle_mouse_event(
        MouseEvent::new_right_press_event(Position::new(6, 16)),
        client,
    );
    let click = plugin_receiver
        .try_iter()
        .find_map(|(instruction, _)| match instruction {
            PluginInstruction::RequestContextMenu { context, .. } => Some(context.click),
            _ => None,
        });
    assert_eq!(click, Some((6, 3)));

    screen.remove_client(other).expect("TEST");
    let (cursor, _) = render_frame_and_cursor_into(&mut screen, &receiver, client, &mut grid);
    assert_eq!(screen.client_presentations[&client].1.offset(), (0, 0));
    assert_eq!(cursor, Some(pane_cursor(&mut screen, client)));
}

#[test]
fn native_client_centers_entire_view_and_mouse_coordinates() {
    let large = Size {
        cols: 101,
        rows: 31,
    };
    let mut screen = create_new_screen(large, true, true);
    let client = ClientId::new(1);
    let other = ClientId::new(2);
    new_tab(&mut screen, 1, TabId::new(0));
    screen.add_client(other).expect("TEST");
    screen
        .set_client_size(other, Size { cols: 80, rows: 24 })
        .expect("TEST");
    screen.resize_to_clients().expect("TEST");
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .handle_pty_bytes(TerminalId::new(1), b"CENTERED".to_vec())
        .unwrap();
    let receiver = screen_wired_to_server(&mut screen);
    screen
        .native_render_clients
        .insert(client, Default::default());
    screen.render_to_clients().unwrap();
    let frames = receiver
        .try_iter()
        .find_map(|(instruction, _)| match instruction {
            ServerInstruction::RenderFrames(frames) => Some(frames),
            _ => None,
        })
        .unwrap();
    let frame = yinz_types::structured_render::decode(&frames[&client]).unwrap();
    let cursor = Some((
        frame.header().cursor_x as usize,
        frame.header().cursor_y as usize,
    ));
    let logical_cursor = pane_cursor(&mut screen, client);
    assert_eq!(cursor, Some((logical_cursor.0 + 10, logical_cursor.1 + 3)));
    assert_eq!(screen.client_presentations[&client].1.offset(), (10, 3));
    assert_eq!(screen.client_presentations[&other].1.offset(), (0, 0));
    assert_eq!(
        screen.client_presentations[&client].1.background,
        screen.style.pane_surround()
    );

    let geometry = frame.geometry().unwrap();
    assert!(geometry
        .panes
        .iter()
        .all(|pane| pane.x >= 10 && pane.y >= 3));
    assert!(frame.rows().any(|(row, cells)| row.y >= 3
        && row.x0 >= 10
        && cells.iter().any(|cell| cell.character() != ' ')));
    let (to_plugin, plugin_receiver) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(to_plugin));
    screen.handle_mouse_event(
        MouseEvent::new_right_press_event(Position::new(0, 0)),
        client,
    );
    assert!(!plugin_receiver.try_iter().any(|(instruction, _)| matches!(
        instruction,
        PluginInstruction::RequestContextMenu { .. }
    )));
    screen.handle_mouse_event(
        MouseEvent::new_right_press_event(Position::new(6, 16)),
        client,
    );
    let click = plugin_receiver
        .try_iter()
        .find_map(|(instruction, _)| match instruction {
            PluginInstruction::RequestContextMenu { context, .. } => Some(context.click),
            _ => None,
        });
    assert_eq!(click, Some((6, 3)));
}

fn centered_mouse_test_screen() -> Screen {
    let mut screen = create_new_screen(
        Size {
            cols: 100,
            rows: 30,
        },
        true,
        true,
    );
    let client = ClientId::new(1);
    new_tab(&mut screen, 1, TabId::new(0));
    screen.add_client(ClientId::new(2)).unwrap();
    screen
        .set_client_size(ClientId::new(2), Size { cols: 80, rows: 24 })
        .unwrap();
    screen.resize_to_clients().unwrap();
    let tab = screen.get_active_tab_mut(client).unwrap();
    tab.vertical_split(
        PaneId::Terminal(TerminalId::new(2)),
        None,
        None,
        client,
        None,
        None,
    )
    .unwrap();
    tab.update_advanced_mouse_actions(true);
    tab.update_mouse_hover_effects(true);
    let receiver = screen_wired_to_server(&mut screen);
    client_frame_bytes(&mut screen, &receiver);
    assert_eq!(screen.client_presentations[&client].1.offset(), (10, 3));
    screen
}

#[test]
fn centered_gutter_motion_preserves_focus_and_reentry_focuses_content() {
    let mut screen = centered_mouse_test_screen();
    let client = ClientId::new(1);
    let left = PaneId::Terminal(TerminalId::new(1));
    let right = PaneId::Terminal(TerminalId::new(2));
    screen
        .get_active_tab_mut(client)
        .unwrap()
        .update_focus_follows_mouse(true);
    assert_eq!(screen.active_pane_id(client), Some(right));

    for position in [
        Position::new(8, 9),
        Position::new(8, 90),
        Position::new(2, 20),
        Position::new(27, 20),
    ] {
        screen.handle_mouse_event(MouseEvent::new_buttonless_motion(position), client);
        assert_eq!(screen.active_pane_id(client), Some(right));
    }
    screen.handle_mouse_event(
        MouseEvent::new_buttonless_motion(Position::new(8, 20)),
        client,
    );
    assert_eq!(screen.active_pane_id(client), Some(left));
}

#[test]
fn centered_gutter_motion_clears_hover_and_reentry_restores_it() {
    let mut screen = centered_mouse_test_screen();
    let client = ClientId::new(1);
    let left = PaneId::Terminal(TerminalId::new(1));
    screen
        .get_active_tab_mut(client)
        .unwrap()
        .update_focus_follows_mouse(false);
    for position in [
        Position::new(8, 9),
        Position::new(8, 90),
        Position::new(2, 20),
        Position::new(27, 20),
    ] {
        screen.handle_mouse_event(
            MouseEvent::new_buttonless_motion(Position::new(8, 20)),
            client,
        );
        assert_eq!(
            screen
                .get_active_tab(client)
                .unwrap()
                .query_mouse_hover_pane_id()
                .get(&client),
            Some(&left),
        );
        screen.handle_mouse_event(MouseEvent::new_buttonless_motion(position), client);
        assert!(!screen
            .get_active_tab(client)
            .unwrap()
            .query_mouse_hover_pane_id()
            .contains_key(&client));
    }
}

#[test]
fn centered_resize_drag_and_release_reach_tab_from_gutter() {
    let mut screen = centered_mouse_test_screen();
    let client = ClientId::new(1);
    let right = PaneId::Terminal(TerminalId::new(2));
    let right_x = |screen: &Screen| {
        screen
            .get_active_tab(client)
            .unwrap()
            .get_pane_with_id(right)
            .unwrap()
            .current_geom()
            .x
    };
    screen
        .get_active_tab_mut(client)
        .unwrap()
        .set_pane_frames(yinz_types::input::options::PaneFrameStyle::Full);
    let initial_x = right_x(&screen);
    screen.handle_mouse_event(
        MouseEvent::new_left_press_with_ctrl_event(Position::new(8, (10 + initial_x) as u16)),
        client,
    );
    screen.handle_mouse_event(
        MouseEvent::new_left_motion_with_ctrl_event(Position::new(2, (11 + initial_x) as u16)),
        client,
    );
    assert_ne!(
        right_x(&screen),
        initial_x,
        "gutter motion must advance the resize"
    );
    screen.handle_mouse_event(
        MouseEvent::new_left_release_with_ctrl_event(Position::new(2, (11 + initial_x) as u16)),
        client,
    );
    let released_x = right_x(&screen);
    screen.handle_mouse_event(
        MouseEvent::new_left_motion_with_ctrl_event(Position::new(8, 60)),
        client,
    );
    assert_eq!(
        right_x(&screen),
        released_x,
        "gutter release must end the resize"
    );
}

#[test]
fn centered_gutters_dismiss_menus_and_clear_menu_hover() {
    use yinz_types::data::{ContextMenuContext, ContextMenuItem};
    use yinz_types::input::actions::Action;
    use yinz_types::input::context_menu::ContextMenu;

    let client = ClientId::new(1);
    for click in [(0, 0), (79, 23)] {
        for position in [
            Position::new(8, 9),
            Position::new(8, 90),
            Position::new(2, 20),
            Position::new(27, 20),
        ] {
            let mut screen = centered_mouse_test_screen();
            let focus = screen.active_pane_id(client);
            let items = vec![ContextMenuItem::new("Copy", vec![Action::Copy])];
            screen.context_menu = ContextMenu::from_items(items.clone());
            let request = screen.open_context_menu(
                ContextMenuContext {
                    pane_id: focus.unwrap(),
                    click,
                    ..Default::default()
                },
                client,
            );
            screen.set_context_menu_items(request, items, None, client);
            let layout = screen
                .context_menu_layout_for(client, screen.client_surface(client).unwrap())
                .unwrap();
            let row = layout.row_of_item(0).unwrap();
            screen.handle_mouse_event(
                MouseEvent::new_buttonless_motion(Position::new(
                    (row + 3) as i32,
                    (layout.x + 2 + 10) as u16,
                )),
                client,
            );
            assert_eq!(screen.context_menus[&client].selected, Some(0));
            screen.handle_mouse_event(MouseEvent::new_buttonless_motion(position), client);
            assert_eq!(screen.context_menus[&client].selected, None);
            screen.handle_mouse_event(MouseEvent::new_left_press_event(position), client);
            assert!(!screen.context_menu_is_open(client));
            assert_eq!(screen.active_pane_id(client), focus);
        }
    }
}

#[test]
fn centered_gutters_dismiss_palettes_and_capture_release() {
    let client = ClientId::new(1);
    for position in [
        Position::new(8, 9),
        Position::new(8, 90),
        Position::new(2, 20),
        Position::new(27, 20),
    ] {
        let mut screen = centered_mouse_test_screen();
        let focus = screen.active_pane_id(client);
        screen.toggle_command_palette(client);
        assert!(screen.command_palettes.contains_key(&client));
        screen.handle_mouse_event(MouseEvent::new_left_press_event(position), client);
        assert!(!screen.command_palettes.contains_key(&client));
        assert!(screen.command_palette_mouse_captures.contains(&client));
        screen.handle_mouse_event(MouseEvent::new_left_release_event(position), client);
        assert!(!screen.command_palette_mouse_captures.contains(&client));
        assert_eq!(screen.active_pane_id(client), focus);
    }
}
