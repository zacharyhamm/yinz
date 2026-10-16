use super::*;

#[test]
fn forward_host_query_when_idle_dispatches_immediately() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(42));
    let query = bg_query();

    let token = screen.forward_host_query(pane_id, query.clone());

    assert_eq!(
        screen.forward_in_flight_token,
        Some(token),
        "slot must flip to in-flight for the dispatched token"
    );
    assert_eq!(
        screen
            .pending_forwarded_queries
            .get(&token)
            .map(|e| e.pane_id),
        Some(pane_id),
        "token→pane mapping must be populated"
    );
    let forwards = capture.drain_forward_queries();
    assert_eq!(forwards.len(), 1, "exactly one forward dispatched");
    assert_eq!(
        forwards[0],
        (token, query.to_query_bytes()),
        "wire bytes must be derived from the HostQuery"
    );
}

#[test]
fn forward_host_query_when_busy_queues_instead_of_dispatching() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    let first_pane = PaneId::Terminal(crate::TerminalId::new(1));
    let second_pane = PaneId::Terminal(crate::TerminalId::new(2));

    let first_token = screen.forward_host_query(first_pane, bg_query());
    let second_token = screen.forward_host_query(second_pane, fg_query());

    // The first call dispatched; the second waits in the queue. Only
    // the first token should be in the map; the second lives in
    // `forward_queue`.
    let forwards = capture.drain_forward_queries();
    assert_eq!(forwards.len(), 1, "second call must not dispatch yet");
    assert_eq!(forwards[0].0, first_token);
    assert_eq!(screen.forward_queue.len(), 1);
    assert_eq!(screen.forward_queue[0].token, second_token);
    assert_eq!(screen.forward_queue[0].pane_id, second_pane);
}

#[test]
fn handle_reply_writes_to_pane_pty_and_releases_slot() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(7));
    let token = screen.forward_host_query(pane_id, bg_query());
    let _ = capture.drain_forward_queries(); // discard the dispatch

    let reply = b"\x1b]11;rgb:1111/2222/3333\x1b\\".to_vec();
    screen
        .handle_forwarded_reply_from_host(token, reply.clone())
        .expect("handler must not fail");

    let writes = capture.drain_pty_writes();
    assert_eq!(writes.len(), 1, "one pty write expected");
    assert_eq!(writes[0], (reply, TerminalId::new(7)));
    assert!(
        screen.forward_in_flight_token.is_none(),
        "slot released so the next queued forward can dispatch"
    );
    assert!(
        !screen.pending_forwarded_queries.contains_key(&token),
        "token entry must have been removed from the map"
    );
}

#[test]
fn handle_reply_dispatches_next_queued_forward() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    let first_pane = PaneId::Terminal(crate::TerminalId::new(1));
    let second_pane = PaneId::Terminal(crate::TerminalId::new(2));
    let first_token = screen.forward_host_query(first_pane, bg_query());
    let second_token = screen.forward_host_query(second_pane, fg_query());
    // First dispatch already emitted; drop it.
    let _ = capture.drain_forward_queries();

    screen
        .handle_forwarded_reply_from_host(first_token, b"reply".to_vec())
        .expect("ok");

    // The queued second forward must now dispatch, and the map now
    // carries the second token → second pane.
    let forwards = capture.drain_forward_queries();
    assert_eq!(forwards.len(), 1, "next queued forward must dispatch");
    assert_eq!(forwards[0].0, second_token);
    assert!(screen.forward_queue.is_empty());
    assert_eq!(screen.forward_in_flight_token, Some(second_token));
    assert_eq!(
        screen
            .pending_forwarded_queries
            .get(&second_token)
            .map(|e| e.pane_id),
        Some(second_pane)
    );
}

#[test]
fn clipboard_read_is_dropped_when_the_option_is_off() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);

    screen.forward_host_query(
        PaneId::Terminal(crate::TerminalId::new(3)),
        clipboard_query(),
    );

    assert!(
        capture.drain_forward_queries().is_empty(),
        "an opted-out clipboard read must never reach the host terminal"
    );
    assert!(screen.clipboard_forward_in_flight_token.is_none());
    assert!(screen.pending_clipboard_forwards.is_empty());
}

#[test]
fn clipboard_read_is_forwarded_asynchronously_when_enabled() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    screen.paste_buffer_read_enabled = true;
    let pane_id = PaneId::Terminal(crate::TerminalId::new(5));

    let token = screen.forward_host_query(pane_id, clipboard_query());

    let forwards = capture.drain_forward_queries_with_async();
    assert_eq!(
        forwards,
        vec![(token, b"\x1b]52;c;?\x1b\\".to_vec(), true)],
        "the query must go out marked as async so the client omits its barrier"
    );
    assert_eq!(screen.clipboard_forward_in_flight_token, Some(token));
    assert_eq!(
        screen
            .pending_clipboard_forwards
            .get(&token)
            .map(|e| e.pane_id),
        Some(pane_id)
    );
    assert!(
        screen.forward_in_flight_token.is_none(),
        "the barrier-serialized slot must stay free"
    );
}

#[test]
fn a_pending_clipboard_read_does_not_block_other_host_queries() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    screen.paste_buffer_read_enabled = true;
    let clipboard_token = screen.forward_host_query(
        PaneId::Terminal(crate::TerminalId::new(1)),
        clipboard_query(),
    );
    let colour_token =
        screen.forward_host_query(PaneId::Terminal(crate::TerminalId::new(2)), bg_query());

    let forwards = capture.drain_forward_queries();
    assert_eq!(
        forwards.len(),
        2,
        "a colour query must dispatch while a clipboard prompt is pending"
    );
    assert_eq!(forwards[0].0, clipboard_token);
    assert_eq!(forwards[1].0, colour_token);
    assert_eq!(screen.forward_in_flight_token, Some(colour_token));
    assert_eq!(
        screen.clipboard_forward_in_flight_token,
        Some(clipboard_token)
    );
}

#[test]
fn a_real_clipboard_reply_is_written_to_the_pane_verbatim() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    screen.paste_buffer_read_enabled = true;
    let token = screen.forward_host_query(
        PaneId::Terminal(crate::TerminalId::new(8)),
        clipboard_query(),
    );
    let _ = capture.drain_forward_queries();

    let reply = b"\x1b]52;c;aGVsbG8=\x1b\\".to_vec();
    screen
        .handle_forwarded_reply_from_host(token, reply.clone())
        .expect("handler must not fail");

    assert_eq!(
        capture.drain_pty_writes(),
        vec![(reply, TerminalId::new(8))]
    );
    assert!(screen.clipboard_forward_in_flight_token.is_none());
    assert!(screen.pending_clipboard_forwards.is_empty());
}

#[test]
fn an_unanswered_clipboard_read_resolves_as_an_empty_clipboard() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    screen.paste_buffer_read_enabled = true;
    let token = screen.forward_host_query(
        PaneId::Terminal(crate::TerminalId::new(9)),
        clipboard_query(),
    );
    let _ = capture.drain_forward_queries();

    screen
        .handle_forwarded_reply_from_host(token, Vec::new())
        .expect("handler must not fail");

    assert_eq!(
        capture.drain_pty_writes(),
        vec![(b"\x1b]52;c;\x1b\\".to_vec(), TerminalId::new(9))]
    );
}

#[test]
fn a_late_clipboard_reply_is_discarded() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    screen.paste_buffer_read_enabled = true;
    let token = screen.forward_host_query(
        PaneId::Terminal(crate::TerminalId::new(9)),
        clipboard_query(),
    );
    let _ = capture.drain_forward_queries();
    screen
        .handle_forwarded_reply_from_host(token, Vec::new())
        .expect("ok");
    let _ = capture.drain_pty_writes();

    screen
        .handle_forwarded_reply_from_host(token, b"\x1b]52;c;bGF0ZQ==\x1b\\".to_vec())
        .expect("ok");

    assert!(
        capture.drain_pty_writes().is_empty(),
        "clipboard data arriving after the window closed must never reach the pane"
    );
}

#[test]
fn a_second_clipboard_read_waits_for_the_first() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    screen.paste_buffer_read_enabled = true;
    let first_token = screen.forward_host_query(
        PaneId::Terminal(crate::TerminalId::new(1)),
        clipboard_query(),
    );
    let second_token = screen.forward_host_query(
        PaneId::Terminal(crate::TerminalId::new(2)),
        clipboard_query(),
    );

    let forwards = capture.drain_forward_queries();
    assert_eq!(
        forwards.len(),
        1,
        "a host can only show one consent prompt at a time"
    );
    assert_eq!(forwards[0].0, first_token);
    assert_eq!(screen.clipboard_forward_queue.len(), 1);
    assert_eq!(screen.clipboard_forward_queue[0].token, second_token);
}

#[test]
fn a_queued_clipboard_read_for_a_closed_pane_is_skipped() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    screen.paste_buffer_read_enabled = true;
    let first_token = screen.forward_host_query(
        PaneId::Terminal(crate::TerminalId::new(1)),
        clipboard_query(),
    );
    screen.forward_host_query(
        PaneId::Terminal(crate::TerminalId::new(2)),
        clipboard_query(),
    );
    let _ = capture.drain_forward_queries();

    screen
        .handle_forwarded_reply_from_host(first_token, Vec::new())
        .expect("ok");

    assert!(
        capture.drain_forward_queries().is_empty(),
        "the queued read belongs to a pane no tab owns"
    );
    assert!(screen.clipboard_forward_queue.is_empty());
    assert!(screen.clipboard_forward_in_flight_token.is_none());
}

#[test]
fn clear_nested_guest_does_not_notify_guest_focus_lost_on_teardown() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(7));
    let client_id = ClientId::new(1);
    screen.nested_guest_choices.insert(
        (client_id, pane_id),
        super::super::NestedGuestChoice::Descend,
    );

    screen.clear_nested_guest(pane_id);

    let notify_flags = capture.drain_key_passthrough_notify_flags();
    assert_eq!(
        notify_flags,
        vec![false],
        "teardown clear must emit KeyPassthroughChanged with notify_guest=false so no FocusLost \
         frame is written to the exiting guest pane"
    );
}

#[test]
fn remove_client_notifies_guest_focus_lost_on_live_ascend() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(7));
    let client_id = ClientId::new(1);
    screen.nested_guest_choices.insert(
        (client_id, pane_id),
        super::super::NestedGuestChoice::Descend,
    );

    screen.remove_client(client_id).expect("remove_client ok");

    let notify_flags = capture.drain_key_passthrough_notify_flags();
    assert_eq!(
        notify_flags,
        vec![true],
        "a live client leaving a still-alive guest must emit notify_guest=true so the guest is \
         told it lost focus"
    );
}

#[test]
fn suspend_nested_guest_preserves_choices_for_later_revival() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, _capture) = create_new_screen_with_forward_capture(size);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(7));
    let client_id = ClientId::new(1);
    screen.nested_guest_choices.insert(
        (client_id, pane_id),
        super::super::NestedGuestChoice::Descend,
    );
    screen
        .nested_guest_tracker
        .on_announce(pane_id, std::time::Instant::now());

    screen.suspend_nested_guest(pane_id);

    assert!(
        screen
            .nested_guest_choices
            .contains_key(&(client_id, pane_id)),
        "suspend must preserve the client's descend choice so a re-announcing guest can be revived \
         into the exact prior state"
    );
}

#[test]
fn clear_nested_guest_discards_choices_unlike_suspend() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, _capture) = create_new_screen_with_forward_capture(size);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(7));
    let client_id = ClientId::new(1);
    screen.nested_guest_choices.insert(
        (client_id, pane_id),
        super::super::NestedGuestChoice::Descend,
    );

    screen.clear_nested_guest(pane_id);

    assert!(
        !screen
            .nested_guest_choices
            .contains_key(&(client_id, pane_id)),
        "a full teardown (Bye/ClosePane) must discard choices, distinguishing it from suspend"
    );
}

#[test]
fn handle_reply_with_unknown_token_is_silent_noop() {
    // Token not in the map AND not the in-flight token: the handler
    // must not panic, must not write any bytes, and crucially must
    // NOT release the slot — the actually-in-flight forward still
    // owns it. (See `late_timeout_after_real_reply_does_not_clobber`
    // for the race scenario this guard prevents.)
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    let first_pane = PaneId::Terminal(crate::TerminalId::new(1));
    let second_pane = PaneId::Terminal(crate::TerminalId::new(2));
    let first_token = screen.forward_host_query(first_pane, bg_query());
    let _second_token = screen.forward_host_query(second_pane, fg_query());
    let _ = capture.drain_forward_queries();

    // Reply for a stale / unknown token (neither first nor second).
    screen
        .handle_forwarded_reply_from_host(9999, b"dropped".to_vec())
        .expect("unknown tokens must not error");

    assert!(
        capture.drain_pty_writes().is_empty(),
        "unknown token must not produce any pty write"
    );
    assert_eq!(
        screen.forward_in_flight_token,
        Some(first_token),
        "in-flight token must still be the original"
    );
    assert!(
        capture.drain_forward_queries().is_empty(),
        "stale reply must not advance the queue"
    );
}

#[test]
fn late_timeout_after_real_reply_does_not_clobber_next_in_flight() {
    // The race the token-equality guard exists to prevent:
    //   1. dispatch token A (slot in-flight = A; A's timer is sleeping).
    //   2. real reply for A arrives → handler releases slot, dispatches
    //      queued token B → slot in-flight = B; B's timer is sleeping.
    //   3. A's server-side timeout fires after the real reply, sending
    //      an empty `ForwardedReplyFromHost { token: A, reply_bytes: [] }`.
    //
    // Without the guard, step 3 would clear the slot for token B and
    // pop the next queued forward, clobbering an actively-in-flight
    // request. The guard makes the late timeout a no-op.
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    let pane_a = PaneId::Terminal(crate::TerminalId::new(1));
    let pane_b = PaneId::Terminal(crate::TerminalId::new(2));
    let pane_c = PaneId::Terminal(crate::TerminalId::new(3));
    let token_a = screen.forward_host_query(pane_a, bg_query());
    let token_b = screen.forward_host_query(pane_b, fg_query());
    let token_c = screen.forward_host_query(pane_c, palette_query(5));
    let _ = capture.drain_forward_queries();

    // Real reply for A arrives → slot moves to B.
    screen
        .handle_forwarded_reply_from_host(token_a, b"real-A".to_vec())
        .expect("ok");
    let dispatched = capture.drain_forward_queries();
    assert_eq!(dispatched.len(), 1, "B must dispatch on A's release");
    assert_eq!(dispatched[0].0, token_b);
    assert_eq!(screen.forward_in_flight_token, Some(token_b));
    // Drain the real reply's pty write so the late-timeout assertion
    // below only sees writes (or absence thereof) caused by step 3.
    let real_writes = capture.drain_pty_writes();
    assert_eq!(real_writes.len(), 1, "real reply should write once");

    // Late timeout for A fires (server-side timer woke up after the
    // real reply already advanced the queue).
    screen
        .handle_forwarded_reply_from_host(token_a, Vec::new())
        .expect("late timeout must be a no-op, not an error");

    assert_eq!(
        screen.forward_in_flight_token,
        Some(token_b),
        "B's slot must NOT be released by A's late timeout"
    );
    assert_eq!(
        screen.forward_queue.front().map(|p| p.token),
        Some(token_c),
        "C must still be queued — A's late timeout must not have popped it"
    );
    assert!(
        capture.drain_forward_queries().is_empty(),
        "no spurious dispatch from a late timeout"
    );
    assert!(
        capture.drain_pty_writes().is_empty(),
        "no synthetic write to any pane from a late timeout"
    );
}

#[test]
fn timeout_for_in_flight_token_releases_slot_with_cache_fallback() {
    // The non-racing case: server-side timeout fires while the token
    // is still in flight (no client reply ever arrived — the old-client
    // compatibility path). The handler must synthesize a cache-derived
    // reply for the pane, release the slot, and dispatch the next
    // queued forward. (Identical externally to the empty-reply
    // cache-fallback case, since the timer fires by sending an empty
    // reply for the in-flight token.)
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    screen.update_terminal_background_color("rgb:1010/2020/3030".to_string());
    let pane = PaneId::Terminal(crate::TerminalId::new(11));
    let queued_pane = PaneId::Terminal(crate::TerminalId::new(12));
    let token = screen.forward_host_query(pane, bg_query());
    let queued_token = screen.forward_host_query(queued_pane, fg_query());
    let _ = capture.drain_forward_queries();

    // Simulate the timeout firing: empty reply for the in-flight token.
    screen
        .handle_forwarded_reply_from_host(token, Vec::new())
        .expect("ok");

    let writes = capture.drain_pty_writes();
    assert_eq!(writes.len(), 1, "cache fallback writes one reply");
    assert_eq!(
        std::str::from_utf8(&writes[0].0).unwrap(),
        "\u{1b}]11;rgb:1010/2020/3030\u{1b}\\",
    );
    assert_eq!(
        screen.forward_in_flight_token,
        Some(queued_token),
        "queued forward dispatched on slot release"
    );
}

#[test]
fn token_counter_wraps_skipping_sentinel() {
    // `next_forward_token == u32::MAX` → first allocation yields
    // `u32::MAX`, the counter then wraps to 0 which is the reserved
    // sentinel and is skipped, so the next allocation yields 1.
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    screen.next_forward_token = u32::MAX;

    let t1 = screen.forward_host_query(PaneId::Terminal(crate::TerminalId::new(1)), bg_query());
    // Clear the in-flight slot so the next call actually allocates
    // (the queueing branch would still allocate, but we want the
    // dispatch branch here).
    screen
        .handle_forwarded_reply_from_host(t1, b"r".to_vec())
        .expect("ok");
    let _ = capture.drain_forward_queries();
    let _ = capture.drain_pty_writes();

    let t2 = screen.forward_host_query(PaneId::Terminal(crate::TerminalId::new(2)), fg_query());
    assert_eq!(t1, u32::MAX, "first token should land on u32::MAX");
    assert_eq!(
        t2, 1,
        "sentinel 0 must be skipped; next allocation wraps to 1"
    );
}

#[test]
fn plugin_pane_reply_is_dropped_without_write() {
    // Plugin panes never emit whitelisted host queries in production
    // code, but if a token → PaneId::Plugin mapping ever lands in the
    // map (via tests or future misuse), the handler must drop the
    // reply rather than routing it to the pty writer (plugin panes
    // don't have a pty).
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    screen.pending_forwarded_queries.insert(
        77,
        super::super::PendingForwardEntry {
            pane_id: PaneId::Plugin(crate::PluginId::new(42)),
            query: bg_query(),
        },
    );
    screen.forward_in_flight_token = Some(77);

    screen
        .handle_forwarded_reply_from_host(77, b"\x1b]11;rgb:0/0/0\x1b\\".to_vec())
        .expect("ok");

    assert!(
        capture.drain_pty_writes().is_empty(),
        "plugin-pane token must not produce a pty write"
    );
    assert!(
        !screen.pending_forwarded_queries.contains_key(&77),
        "map entry must still be cleared even when the reply is dropped"
    );
    assert!(
        screen.forward_in_flight_token.is_none(),
        "slot still released"
    );
}

// =====================================================================
// Cache-fallback synthesis: empty reply → answer from Screen's caches
// =====================================================================

#[test]
fn empty_reply_falls_back_to_cached_background() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    let pane = PaneId::Terminal(crate::TerminalId::new(9));
    screen.update_terminal_background_color("rgb:1010/2020/3030".to_string());
    let token = screen.forward_host_query(pane, bg_query());
    let _ = capture.drain_forward_queries();

    // Empty reply — the client couldn't or didn't answer.
    screen
        .handle_forwarded_reply_from_host(token, Vec::new())
        .expect("ok");

    let writes = capture.drain_pty_writes();
    assert_eq!(writes.len(), 1, "synthesis must write exactly one reply");
    let (bytes, terminal_id) = &writes[0];
    assert_eq!(*terminal_id, crate::TerminalId::new(9));
    assert_eq!(
        std::str::from_utf8(bytes).unwrap(),
        "\u{1b}]11;rgb:1010/2020/3030\u{1b}\\",
    );
}

#[test]
fn empty_reply_falls_back_to_cached_foreground_with_bel_terminator() {
    // Query used BEL; reply must mirror the same terminator.
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    let pane = PaneId::Terminal(crate::TerminalId::new(1));
    screen.update_terminal_foreground_color("rgb:dcdc/dcdc/dcdc".to_string());
    let token = screen.forward_host_query(pane, fg_query_bel());
    let _ = capture.drain_forward_queries();

    screen
        .handle_forwarded_reply_from_host(token, Vec::new())
        .expect("ok");

    let writes = capture.drain_pty_writes();
    assert_eq!(writes.len(), 1);
    assert_eq!(
        std::str::from_utf8(&writes[0].0).unwrap(),
        "\u{1b}]10;rgb:dcdc/dcdc/dcdc\u{7}",
    );
}

#[test]
fn empty_reply_falls_back_to_cached_pixel_dimensions() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    let pane = PaneId::Terminal(crate::TerminalId::new(1));
    screen.update_pixel_dimensions(
        crate::ClientId::new(1),
        PixelDimensions {
            text_area_grid_size: None,
            character_cell_size: Some(SizeInPixels {
                height: 19,
                width: 9,
            }),
            text_area_size: Some(SizeInPixels {
                height: 608,
                width: 931,
            }),
        },
    );

    // CSI 14t — text-area pixels.
    let token = screen.forward_host_query(pane, HostQuery::TextAreaPixelSize);
    let _ = capture.drain_forward_queries();
    screen
        .handle_forwarded_reply_from_host(token, Vec::new())
        .expect("ok");
    let writes = capture.drain_pty_writes();
    assert_eq!(
        std::str::from_utf8(&writes[0].0).unwrap(),
        "\u{1b}[4;608;931t"
    );

    // CSI 16t — cell size.
    let token = screen.forward_host_query(pane, HostQuery::CharacterCellPixelSize);
    let _ = capture.drain_forward_queries();
    screen
        .handle_forwarded_reply_from_host(token, Vec::new())
        .expect("ok");
    let writes = capture.drain_pty_writes();
    assert_eq!(std::str::from_utf8(&writes[0].0).unwrap(), "\u{1b}[6;19;9t");
}

#[test]
fn empty_reply_falls_back_to_cached_palette_register() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    let pane = PaneId::Terminal(crate::TerminalId::new(1));
    screen.update_terminal_color_registers(vec![(42, "rgb:abab/cdcd/efef".to_string())]);
    let token = screen.forward_host_query(pane, palette_query(42));
    let _ = capture.drain_forward_queries();

    screen
        .handle_forwarded_reply_from_host(token, Vec::new())
        .expect("ok");

    let writes = capture.drain_pty_writes();
    assert_eq!(writes.len(), 1);
    assert_eq!(
        std::str::from_utf8(&writes[0].0).unwrap(),
        "\u{1b}]4;42;rgb:abab/cdcd/efef\u{1b}\\",
    );
}

#[test]
fn empty_reply_with_no_cache_writes_empty() {
    // No background override, no palette, no pixel dims: synthesis
    // returns empty and the pane receives an empty write — the app
    // decides what to do with "host declined".
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    let pane = PaneId::Terminal(crate::TerminalId::new(1));

    // Default Palette::fg is EightBit(0), not Rgb — synthesis refuses.
    let token = screen.forward_host_query(pane, fg_query());
    let _ = capture.drain_forward_queries();
    screen
        .handle_forwarded_reply_from_host(token, Vec::new())
        .expect("ok");
    let writes = capture.drain_pty_writes();
    assert_eq!(writes.len(), 1, "still writes — the write is empty bytes");
    assert!(
        writes[0].0.is_empty(),
        "no rgb cache → synthesis returns empty"
    );
}

#[test]
fn non_empty_reply_bypasses_synthesis() {
    // A real reply from the host must be passed through verbatim —
    // we must not second-guess it with cached state.
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    let pane = PaneId::Terminal(crate::TerminalId::new(1));
    screen.update_terminal_background_color("rgb:0000/0000/0000".to_string());
    let token = screen.forward_host_query(pane, bg_query());
    let _ = capture.drain_forward_queries();

    let real = b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\".to_vec();
    screen
        .handle_forwarded_reply_from_host(token, real.clone())
        .expect("ok");

    let writes = capture.drain_pty_writes();
    assert_eq!(writes.len(), 1);
    assert_eq!(
        writes[0].0, real,
        "real reply must be forwarded verbatim, not replaced by cached bg"
    );
}

#[test]
fn codex_palette_observes_and_retains_local_forwarded_and_cached_color_replies() {
    use base64::Engine;
    use yinz_types::agent::theme::*;
    for mode in ["local", "forwarded", "fallback"] {
        let (mut screen, capture) =
            create_new_screen_with_forward_capture(Size { cols: 80, rows: 20 });
        new_tab(&mut screen, 1, TabId::new(0));
        let pane_id = PaneId::Terminal(TerminalId::new(1));
        let snapshot = AgentThemeSnapshot {
            version: SNAPSHOT_VERSION,
            provider: "codex".into(),
            provider_version: "0.162.1".into(),
            theme: "automatic".into(),
            provenance: vec![],
            availability: Availability::Available,
            definitions: vec![],
            collisions: vec![],
        };
        let event = AgentThemeEvent {
            version: EVENT_VERSION,
            launch_id: "codex-colors".into(),
            snapshot: Some(snapshot),
        };
        let encoded =
            base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&event).unwrap());
        screen
            .tab_containing_pane_mut(pane_id)
            .unwrap()
            .get_pane_with_id_mut(pane_id)
            .unwrap()
            .handle_pty_bytes(format!("\x1b]777;yinz-agent-theme;{encoded}\x1b\\").into_bytes());
        screen.update_terminal_foreground_color("rgb:dcdc/dcdc/dcdc".into());
        screen.update_terminal_background_color("rgb:1212/1414/1e1e".into());
        if mode == "local" {
            screen
                .tab_containing_pane_mut(pane_id)
                .unwrap()
                .handle_pty_bytes(
                    TerminalId::new(1),
                    b"\x1b]10;rgb:dcdc/dcdc/dcdc\x07\x1b]11;rgb:1212/1414/1e1e\x07\x1b]10;?;?\x07"
                        .to_vec(),
                )
                .unwrap();
        } else {
            for (query_bytes, reply) in [
                (
                    b"\x1b]10;?\x07".as_slice(),
                    b"\x1b]10;rgb:dcdc/dcdc/dcdc\x07".as_slice(),
                ),
                (
                    b"\x1b]11;?\x07".as_slice(),
                    b"\x1b]11;rgb:1212/1414/1e1e\x07".as_slice(),
                ),
            ] {
                let pane = screen
                    .tab_containing_pane_mut(pane_id)
                    .unwrap()
                    .get_pane_with_id_mut(pane_id)
                    .unwrap();
                pane.handle_pty_bytes(query_bytes.to_vec());
                let queries = pane.drain_forwarded_queries();
                assert_eq!(queries.len(), 1);
                let token = screen.forward_host_query(pane_id, queries[0].clone());
                screen
                    .handle_forwarded_reply_from_host(
                        token,
                        if mode == "fallback" {
                            vec![]
                        } else {
                            reply.to_vec()
                        },
                    )
                    .unwrap();
                // Duplicate replies (including timeout callbacks) must not change the sample.
                screen
                    .handle_forwarded_reply_from_host(
                        token,
                        b"\x1b]11;rgb:ffff/ffff/ffff\x07".to_vec(),
                    )
                    .unwrap();
            }
        }
        let pane = screen
            .tab_containing_pane_mut(pane_id)
            .unwrap()
            .get_pane_with_id_mut(pane_id)
            .unwrap();
        let (_, metadata) = pane.serialize_handoff(None, 100_000).unwrap();
        let metadata = metadata.unwrap();
        let snapshot = &metadata.snapshots[metadata.active.unwrap().palette];
        assert!(snapshot
            .definitions
            .iter()
            .any(|d| d.token == "codex.terminal.composer-plan"
                && d.color == SourceColor::Rgb(46, 48, 57)
                && d.target == Some(Role::Surface.into())));
        // No output followed either reply: resume must still drain observations.
        let updates: Vec<_> = capture
            .pty_writer_rx
            .try_iter()
            .filter_map(|(instruction, _)| match instruction {
                PtyWriteInstruction::RetainAgentTheme(id, event) => {
                    assert_eq!(PaneId::Terminal(id), pane_id);
                    Some(event)
                },
                _ => None,
            })
            .collect();
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[1].snapshot.as_ref(), Some(snapshot));
        assert!(pane.drain_agent_theme_updates().is_empty());
    }
}
