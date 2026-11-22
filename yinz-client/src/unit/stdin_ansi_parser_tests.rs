//! Unit tests for the continuous host-reply parser.

use super::{schedule_timeout, ForwardKind, HostReply, PendingPartial, StdinAnsiParser};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// What a forward-timeout test captures: the pane the bytes were forwarded to, and the bytes.
type CapturedForward = Arc<Mutex<Option<(u32, Vec<u8>)>>>;

/// One host-reply case: the bytes to feed, a predicate over the reply, and a label.
type HostReplyCase = (&'static [u8], fn(&HostReply) -> bool, &'static str);

/// Helper: collect replies and residue from a single `feed` call.
fn feed_once(parser: &mut StdinAnsiParser, bytes: &[u8]) -> (Vec<HostReply>, Vec<u8>) {
    let out = parser.feed(bytes);
    (out.replies, out.residue)
}

#[test]
fn pixel_dimensions_text_area_reply() {
    // CSI 4 ; H ; W t
    let mut parser = StdinAnsiParser::new();
    let (replies, residue) = feed_once(&mut parser, b"\x1b[4;720;1280t");
    assert!(
        residue.is_empty(),
        "pixel-dim reply should be fully consumed"
    );
    assert_eq!(replies.len(), 1);
    match &replies[0] {
        HostReply::PixelDimensions(pd) => {
            let tas = pd.text_area_size.expect("text area size");
            assert_eq!(tas.height, 720);
            assert_eq!(tas.width, 1280);
            assert!(pd.character_cell_size.is_none());
        },
        other => panic!("unexpected reply: {:?}", other),
    }
}

#[test]
fn pixel_dimensions_character_cell_reply() {
    let mut parser = StdinAnsiParser::new();
    let (replies, residue) = feed_once(&mut parser, b"\x1b[6;18;9t");
    assert!(residue.is_empty());
    match &replies[0] {
        HostReply::PixelDimensions(pd) => {
            let ccs = pd.character_cell_size.expect("cell size");
            assert_eq!(ccs.height, 18);
            assert_eq!(ccs.width, 9);
        },
        other => panic!("unexpected reply: {:?}", other),
    }
}

#[test]
fn background_color_reply() {
    let mut parser = StdinAnsiParser::new();
    let (replies, residue) = feed_once(&mut parser, b"\x1b]11;rgb:0000/0000/0000\x1b\\");
    assert!(residue.is_empty());
    match &replies[0] {
        HostReply::BackgroundColor(s) => assert_eq!(s, "rgb:0000/0000/0000"),
        other => panic!("unexpected reply: {:?}", other),
    }
}

#[test]
fn foreground_color_reply() {
    let mut parser = StdinAnsiParser::new();
    let (replies, residue) = feed_once(&mut parser, b"\x1b]10;rgb:ffff/ffff/ffff\x1b\\");
    assert!(residue.is_empty());
    match &replies[0] {
        HostReply::ForegroundColor(s) => assert_eq!(s, "rgb:ffff/ffff/ffff"),
        other => panic!("unexpected reply: {:?}", other),
    }
}

#[test]
fn color_register_reply() {
    let mut parser = StdinAnsiParser::new();
    let (replies, residue) = feed_once(&mut parser, b"\x1b]4;5;rgb:8080/8080/8080\x1b\\");
    assert!(residue.is_empty());
    match &replies[0] {
        HostReply::ColorRegisters(regs) => {
            assert_eq!(regs.len(), 1);
            assert_eq!(regs[0].0, 5);
            assert_eq!(regs[0].1, "rgb:8080/8080/8080");
        },
        other => panic!("unexpected reply: {:?}", other),
    }
}

#[test]
fn synchronized_output_supported_reply() {
    let mut parser = StdinAnsiParser::new();
    let (replies, residue) = feed_once(&mut parser, b"\x1b[?2026;1$y");
    assert!(residue.is_empty());
    match &replies[0] {
        HostReply::SynchronizedOutput(Some(_)) => {},
        other => panic!("unexpected reply: {:?}", other),
    }
}

#[test]
fn keyboard_residue_passes_through_unchanged() {
    // Arrow key escape sequence is NOT a whitelisted CSI report (final
    // byte 'A'), so it must survive as keyboard residue verbatim.
    let mut parser = StdinAnsiParser::new();
    let (replies, residue) = feed_once(&mut parser, b"\x1b[A");
    assert!(replies.is_empty());
    assert_eq!(residue, b"\x1b[A");
}

#[test]
fn mixed_keyboard_and_reply_extracts_both_cleanly() {
    // Arrow keys bracketing a pixel-dim reply — residue should be just
    // the arrow-key bytes, reply should be classified.
    let mut parser = StdinAnsiParser::new();
    let mut input = Vec::new();
    input.extend_from_slice(b"\x1b[A");
    input.extend_from_slice(b"\x1b[4;720;1280t");
    input.extend_from_slice(b"\x1b[B");
    let (replies, residue) = feed_once(&mut parser, &input);
    assert_eq!(replies.len(), 1);
    matches!(replies[0], HostReply::PixelDimensions(_));
    assert_eq!(residue, b"\x1b[A\x1b[B");
}

#[test]
fn unterminated_osc_within_single_chunk_is_buffered() {
    // An OSC that never terminates within a chunk is held in the
    // partial-OSC buffer for the next call to complete; nothing leaks
    // into residue. This is the cross-chunk-aware replacement for the
    // pre-fix behaviour where unterminated OSC bytes fell through to
    // residue and surfaced as spurious keypresses.
    let mut parser = StdinAnsiParser::new();
    let (_replies, residue) = feed_once(&mut parser, b"\x1b]10;partial");
    assert!(
        residue.is_empty(),
        "unterminated OSC must be buffered, not leaked to residue: {:?}",
        residue
    );
}

#[test]
fn forwarding_window_accumulates_and_barrier_closes() {
    let mut parser = StdinAnsiParser::new();
    parser.open_forward(42);
    // Feed an OSC 11 reply, then the Primary-DA barrier. Use color
    // bytes that do NOT contain `c` so the barrier-absence assertion
    // below can use a simple byte search.
    let mut chunk = Vec::new();
    chunk.extend_from_slice(b"\x1b]11;rgb:aaaa/bbbb/dddd\x1b\\");
    chunk.extend_from_slice(b"\x1b[?65;1c");
    let out = parser.feed(&chunk);
    // OSC 11 was classified (double-dispatch), and the barrier itself
    // advertises the host's sixel capability.
    assert_eq!(out.replies.len(), 2);
    matches!(out.replies[0], HostReply::BackgroundColor(_));
    matches!(out.replies[1], HostReply::SixelSupport(false));
    // Barrier closed the window, producing a completed forward.
    let (token, reply_bytes) = out
        .completed_forward
        .expect("barrier should close the window");
    assert_eq!(token, 42);
    // Reply bytes should contain the OSC 11 (re-serialized) but NOT the
    // barrier reply itself.
    assert!(
        reply_bytes.windows(5).any(|w| w == b"]11;r"),
        "OSC 11 should be in the forwarded buffer: {:?}",
        reply_bytes
    );
    assert!(
        !reply_bytes.contains(&b'c'),
        "Primary-DA barrier (final byte 'c') must not appear in forwarded reply"
    );
    // Slot released.
    assert!(parser.active_forward_token().is_none());
}

#[test]
fn unsolicited_osc_between_forwarded_query_and_barrier() {
    // Scenario: host emits a stray OSC 10 between the app's OSC 11 query
    // and the barrier. Both replies should end up in the forwarded
    // buffer, and the barrier closes the window.
    let mut parser = StdinAnsiParser::new();
    parser.open_forward(7);
    let mut chunk = Vec::new();
    chunk.extend_from_slice(b"\x1b]11;rgb:1111/1111/1111\x1b\\");
    chunk.extend_from_slice(b"\x1b]10;rgb:2222/2222/2222\x1b\\");
    chunk.extend_from_slice(b"\x1b[c");
    let out = parser.feed(&chunk);
    assert_eq!(out.replies.len(), 2);
    let (token, reply_bytes) = out.completed_forward.unwrap();
    assert_eq!(token, 7);
    // Both OSCs present.
    assert!(reply_bytes.windows(4).any(|w| w == b"]11;"));
    assert!(reply_bytes.windows(4).any(|w| w == b"]10;"));
}

#[test]
fn double_dispatch_without_active_forward_still_emits_reply() {
    let mut parser = StdinAnsiParser::new();
    // No open_forward — reply should still be classified.
    let out = parser.feed(b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\");
    assert_eq!(out.replies.len(), 1);
    matches!(out.replies[0], HostReply::BackgroundColor(_));
    assert!(out.completed_forward.is_none());
}

#[test]
fn timeout_flushes_accumulated_bytes() {
    let mut parser = StdinAnsiParser::new();
    parser.open_forward(99);
    let out = parser.feed(b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\");
    assert!(out.completed_forward.is_none(), "no barrier yet");
    assert!(parser.active_forward_token() == Some(99));
    // Simulate timeout firing on the watcher.
    let flushed = parser.close_forward_on_timeout(99);
    let (token, bytes) = flushed.expect("timeout flush should produce a payload");
    assert_eq!(token, 99);
    assert!(bytes.windows(4).any(|w| w == b"]11;"));
    assert!(parser.active_forward_token().is_none());
}

#[test]
fn stale_token_timeout_does_nothing() {
    let mut parser = StdinAnsiParser::new();
    parser.open_forward(1);
    // Ask to timeout a different token — nothing happens.
    assert!(parser.close_forward_on_timeout(999).is_none());
    assert_eq!(parser.active_forward_token(), Some(1));
}

#[test]
fn fragmented_osc_does_not_leak_into_residue() {
    let full = b"\x1b]11;rgb:0000/0000/0000\x1b\\";
    for split in 1..full.len() {
        let mut p = StdinAnsiParser::new();
        let r1 = p.feed(&full[..split]);
        let r2 = p.feed(&full[split..]);
        assert!(
            r1.residue.is_empty(),
            "split at {}: chunk 1 residue should be empty, got {:?}",
            split,
            r1.residue
        );
        assert!(
            r2.residue.is_empty(),
            "split at {}: chunk 2 residue should be empty, got {:?}",
            split,
            r2.residue
        );
        assert_eq!(
            r1.replies.len() + r2.replies.len(),
            1,
            "split at {}: exactly one reply across both chunks",
            split
        );
    }
}

#[test]
fn fragmented_csi_report_does_not_leak() {
    // Pixel-dimensions reply (final byte 't').
    let full = b"\x1b[4;800;1200t";
    for split in 1..full.len() {
        let mut p = StdinAnsiParser::new();
        let r1 = p.feed(&full[..split]);
        let r2 = p.feed(&full[split..]);
        assert!(
            r1.residue.is_empty(),
            "split at {}: c1 residue {:?}",
            split,
            r1.residue
        );
        assert!(
            r2.residue.is_empty(),
            "split at {}: c2 residue {:?}",
            split,
            r2.residue
        );
        assert_eq!(r1.replies.len() + r2.replies.len(), 1);
    }
}

#[test]
fn fragmented_osc_byte_by_byte() {
    // Every byte in its own chunk — the worst case.
    let full = b"\x1b]11;rgb:abcd/ef01/2345\x1b\\";
    let mut p = StdinAnsiParser::new();
    let mut total_replies = 0;
    for &b in full {
        let out = p.feed(&[b]);
        assert!(out.residue.is_empty(), "byte 0x{:02x} leaked to residue", b);
        total_replies += out.replies.len();
    }
    assert_eq!(total_replies, 1);
}

#[test]
fn lone_trailing_esc_is_buffered_then_finalized_as_residue() {
    // A bare ESC byte at the tail of a chunk could be the start of an
    // OSC or CSI host-reply that's been fragmented at the ESC
    // boundary, so `feed` parks it under partial state instead of
    // leaking it as a keyboard residue. But the byte must not stay
    // parked forever — `finalize()` is the idle drain that releases
    // it back to the keyboard parser when no follow-up arrives.
    let mut p = StdinAnsiParser::new();
    let out = p.feed(b"\x1b");
    assert!(
        out.residue.is_empty(),
        "lone ESC must not leak immediately: {:?}",
        out.residue
    );
    assert!(
        out.has_partial_state,
        "lone ESC must mark has_partial_state so the caller schedules a finalize tick"
    );
    assert_eq!(p.pending_partial(), PendingPartial::LoneEsc);
    let drained = p.finalize_fast_partial();
    assert_eq!(
        drained,
        vec![0x1b],
        "finalize must release the parked ESC as keyboard residue"
    );
    // Subsequent finalize is a no-op once the parker is empty.
    assert!(p.finalize_fast_partial().is_empty());
    assert_eq!(p.pending_partial(), PendingPartial::None);
}

#[test]
fn fragmented_osc_does_not_finalize_partial() {
    // Inverse case: a real fragmented host-reply (ESC then `]...`
    // arrived in two chunks with no idle between them) must NOT be
    // released by finalize. The key signal is timing — if the second
    // chunk arrives quickly enough the idle path is never taken, and
    // the OSC completes normally. We exercise that order here.
    let full = b"\x1b]11;rgb:0000/0000/0000\x1b\\";
    let mut p = StdinAnsiParser::new();
    let r1 = p.feed(&full[..1]);
    assert!(r1.residue.is_empty());
    assert!(r1.has_partial_state);
    let r2 = p.feed(&full[1..]);
    assert!(r2.residue.is_empty(), "tail must complete the OSC");
    assert_eq!(r1.replies.len() + r2.replies.len(), 1);
    assert!(
        p.finalize_force().is_empty(),
        "no partial left after completion"
    );
}

#[test]
fn partial_buffer_boundaries_and_recovery() {
    for prefix in [b"\x1b]52;c;".as_slice(), b"\x1b[?", b"\x1b_G"] {
        let mut p = StdinAnsiParser::new();
        p.partial_buffer_limit = 16;
        p.open_forward(42);
        let mut bytes = prefix.to_vec();
        bytes.resize(15, b'1');
        let below = p.feed(&bytes);
        assert!(below.residue.is_empty());
        assert!(below.has_partial_state);
        let exact = p.feed(b"1");
        bytes.push(b'1');
        assert!(exact.residue.is_empty());
        assert!(exact.has_partial_state);
        let overflow = p.feed(b"1");
        bytes.push(b'1');
        assert_eq!(overflow.residue, bytes);
        assert!(!overflow.has_partial_state);
        assert!(p.finalize_force().is_empty());
        assert_eq!(p.active_forward_token(), Some(42));
        assert_eq!(p.feed(b"keyboard").residue, b"keyboard");
        // A late terminator must not complete the abandoned OSC in the decoder.
        let late = p.feed(b"\x07");
        assert_eq!(late.residue, b"\x07");
        assert!(late.replies.is_empty());
        let reply = p.feed(b"\x1b]11;rgb:0000/0000/0000\x07\x1b[?1;2c");
        assert!(reply.residue.is_empty());
        assert!(
            matches!(reply.replies.as_slice(), [HostReply::BackgroundColor(color), HostReply::SixelSupport(false)] if color == "rgb:0000/0000/0000")
        );
        assert_eq!(
            reply.completed_forward,
            Some((42, b"\x1b]11;rgb:0000/0000/0000\x1b\\".to_vec()))
        );
        assert!(p.finalize_force().is_empty());
    }
}

#[test]
fn osc_99_routes_into_desktop_notifications() {
    // OSC 99 is the desktop-notification response. It must surface in
    // ParseOutput.desktop_notifications (where the stdin handler routes
    // it as InputInstruction::DesktopNotificationResponse), NOT in
    // residue (which would never reach the keyboard parser anyway,
    // since the scrubber strips OSC bytes).
    let mut p = StdinAnsiParser::new();
    let out = p.feed(b"\x1b]99;notification body\x1b\\");
    assert!(out.residue.is_empty(), "OSC 99 must not leak to residue");
    assert!(out.replies.is_empty(), "OSC 99 is not a HostReply variant");
    assert_eq!(out.desktop_notifications.len(), 1);
    assert_eq!(out.desktop_notifications[0], b"notification body".to_vec());
}

#[test]
fn osc_99_does_not_leak_into_an_open_forwarding_window() {
    // A notification response arriving while a forward slot is open is
    // consumed as a desktop notification only; the pane that asked for
    // the forwarded reply must not see it.
    let mut parser = StdinAnsiParser::new();
    parser.open_forward(7);
    let mut chunk = Vec::new();
    chunk.extend_from_slice(b"\x1b]99;i=p1.myid;\x1b\\");
    chunk.extend_from_slice(b"\x1b]11;rgb:1111/1111/1111\x1b\\");
    chunk.extend_from_slice(b"\x1b[c");
    let out = parser.feed(&chunk);
    assert_eq!(out.desktop_notifications.len(), 1);
    let (token, reply_bytes) = out.completed_forward.unwrap();
    assert_eq!(token, 7);
    assert!(
        reply_bytes.windows(4).any(|w| w == b"]11;"),
        "the reply the pane asked for is forwarded: {:?}",
        reply_bytes
    );
    assert!(
        !reply_bytes.windows(4).any(|w| w == b"]99;"),
        "the notification response is not: {:?}",
        reply_bytes
    );
}

#[test]
fn fragmented_osc_99_emits_one_notification() {
    // Cross-chunk regression: OSC 99 split across two feed() calls
    // must still emit exactly one notification, with no leak into
    // residue.
    let full = b"\x1b]99;hello world\x1b\\";
    for split in 1..full.len() {
        let mut p = StdinAnsiParser::new();
        let r1 = p.feed(&full[..split]);
        let r2 = p.feed(&full[split..]);
        assert!(r1.residue.is_empty(), "split at {}: c1 residue", split);
        assert!(r2.residue.is_empty(), "split at {}: c2 residue", split);
        let total = r1.desktop_notifications.len() + r2.desktop_notifications.len();
        assert_eq!(total, 1, "split at {}: exactly one notification", split);
    }
}

#[test]
fn malformed_osc_still_does_not_eat_following_keyboard_bytes() {
    // Pre-existing invariant. An unterminated OSC followed (in a later
    // chunk) by plain keyboard input — the keyboard input must reach
    // residue intact once the OSC closes via proper flush.
    let mut p = StdinAnsiParser::new();
    let _ = p.feed(b"\x1b]10;partial");
    let _ = p.feed(b"\x1b\\");
    let out = p.feed(b"hello");
    assert_eq!(out.residue, b"hello");
}

#[test]
fn cross_chunk_osc_assembles_across_feeds() {
    // An OSC 11 reply split across two `feed()` calls. The inner
    // `InputParser` buffers OSC state across calls, so the second
    // chunk's terminator completes the sequence and the parser
    // emits a single classified reply. The first chunk produces no
    // reply; what lands in residue on the first chunk is byte-level
    // scrubber behaviour and intentionally not pinned here (see the
    // follow-up chunked_residue tests for those shapes).
    let mut parser = StdinAnsiParser::new();
    let first = parser.feed(b"\x1b]11;rgb:ffff/");
    assert!(
        first.replies.is_empty(),
        "first chunk must not classify a reply yet: {:?}",
        first.replies
    );

    let second = parser.feed(b"ffff/ffff\x1b\\");
    assert_eq!(
        second.replies.len(),
        1,
        "second chunk completes the OSC; parser emits one reply: {:?}",
        second.replies
    );
    match &second.replies[0] {
        HostReply::BackgroundColor(s) => assert_eq!(s, "rgb:ffff/ffff/ffff"),
        other => panic!("expected BackgroundColor, got {:?}", other),
    }
}

#[test]
fn double_dispatch_matrix_with_forward_active() {
    // For each whitelisted reply variant, opening a forward window and
    // feeding the reply must (a) classify the variant into
    // `ParseOutput.replies` and (b) accumulate the raw bytes into the
    // forward's reply buffer — both paths always fire, so cached state
    // and the forwarded-to pane stay in sync. OSC 11 is already
    // covered by `forwarding_window_accumulates_and_barrier_closes`;
    // this test sweeps OSC 10, OSC 4, CSI 14t / 16t replies, and
    // DECRPM 2026.
    let cases: Vec<HostReplyCase> = vec![
        (
            b"\x1b]10;rgb:1111/2222/3333\x1b\\",
            |r| matches!(r, HostReply::ForegroundColor(_)),
            "OSC 10",
        ),
        (
            b"\x1b]4;9;rgb:4444/5555/6666\x1b\\",
            |r| matches!(r, HostReply::ColorRegisters(_)),
            "OSC 4",
        ),
        (
            b"\x1b[4;720;1280t",
            |r| matches!(r, HostReply::PixelDimensions(_)),
            "CSI 14t reply",
        ),
        (
            b"\x1b[6;18;9t",
            |r| matches!(r, HostReply::PixelDimensions(_)),
            "CSI 16t reply",
        ),
        (
            b"\x1b[?2026;1$y",
            |r| matches!(r, HostReply::SynchronizedOutput(_)),
            "DECRPM 2026",
        ),
    ];
    for (bytes, is_expected_variant, label) in cases {
        let mut parser = StdinAnsiParser::new();
        parser.open_forward(11);
        let out = parser.feed(bytes);
        assert_eq!(
            out.replies.len(),
            1,
            "{}: should classify exactly one reply (got {:?})",
            label,
            out.replies
        );
        assert!(
            is_expected_variant(&out.replies[0]),
            "{}: wrong variant {:?}",
            label,
            out.replies[0]
        );
        assert!(
            out.completed_forward.is_none(),
            "{}: no barrier yet, slot must stay open",
            label
        );
        // Close the window to inspect the forward buffer.
        let (token, raw) = parser
            .close_forward_on_timeout(11)
            .expect("forward slot should still be open");
        assert_eq!(token, 11, "{}: token preserved", label);
        assert!(
            !raw.is_empty(),
            "{}: reply bytes must have been accumulated into the forward buffer",
            label
        );
    }
}

#[test]
fn primary_da_barrier_accepts_extended_forms() {
    // The barrier is "any CSI reply with final byte `c`". Real hosts
    // emit parameters (`\x1b[?62;1;6c`) or the secondary-DA-esque
    // prefix (`\x1b[>0;276;0c`). Both must close the forward window
    // the same way the bare `\x1b[c` does.
    for barrier in [
        b"\x1b[c".as_ref(),
        b"\x1b[?62;1;6c".as_ref(),
        b"\x1b[>0;276;0c".as_ref(),
    ] {
        let mut parser = StdinAnsiParser::new();
        parser.open_forward(5);
        let mut chunk = Vec::new();
        chunk.extend_from_slice(b"\x1b]11;rgb:aaaa/bbbb/cccc\x1b\\");
        chunk.extend_from_slice(barrier);
        let out = parser.feed(&chunk);
        let (token, reply_bytes) = out
            .completed_forward
            .expect("every Primary-DA reply form must close the forward window");
        assert_eq!(token, 5);
        assert!(
            reply_bytes.windows(4).any(|w| w == b"]11;"),
            "OSC 11 should be present in the forwarded buffer for barrier {:?}",
            std::str::from_utf8(barrier).unwrap_or("<non-utf8>")
        );
        assert!(parser.active_forward_token().is_none());
    }
}

// =====================================================================
// Re-entry guard on `open_forward`
// =====================================================================

#[test]
#[should_panic(expected = "while slot for token")]
fn open_forward_debug_asserts_on_reentry() {
    // In debug builds (which is where tests run) the guard fires. This
    // catches a misbehaving server that dispatches a second forward
    // before receiving the first's completion — a scenario that should
    // be impossible given `forward_in_flight` serialization, but the
    // parser asserts it anyway so regressions surface in CI.
    let mut parser = StdinAnsiParser::new();
    parser.open_forward(1);
    parser.open_forward(2); // panics via debug_assert!
}

// =====================================================================
// `schedule_timeout` — async timer, driven by paused tokio clock
// =====================================================================

/// Build a paused current-thread runtime with `enable_time()`. With
/// `start_paused(true)` the clock only advances when we explicitly
/// call `tokio::time::advance`, so tests don't wall-clock sleep.
fn paused_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .expect("paused runtime build must succeed")
}

#[test]
fn timer_fires_after_deadline_and_closes_slot() {
    let rt = paused_runtime();
    let parser = Arc::new(Mutex::new(StdinAnsiParser::new()));
    parser.lock().unwrap().open_forward(7);

    let captured: CapturedForward = Arc::new(Mutex::new(None));
    let captured_clone = captured.clone();
    schedule_timeout(
        rt.handle(),
        parser.clone(),
        ForwardKind::Barrier,
        7,
        Duration::from_millis(500),
        move |token, bytes| {
            *captured_clone.lock().unwrap() = Some((token, bytes));
        },
    );

    // Drive the runtime enough to run the just-spawned task up to its
    // sleep point, then jump past the deadline.
    rt.block_on(async {
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(600)).await;
        // One more yield so the woken task can complete synchronously.
        tokio::task::yield_now().await;
    });

    let (token, bytes) = captured
        .lock()
        .unwrap()
        .take()
        .expect("timer must have invoked on_timeout");
    assert_eq!(token, 7);
    assert!(
        bytes.is_empty(),
        "nothing was fed to the parser → buffer is empty"
    );
    assert!(
        parser.lock().unwrap().active_forward_token().is_none(),
        "slot must have been cleared by the timer"
    );
}

#[test]
fn timer_is_noop_when_barrier_already_closed_the_slot() {
    let rt = paused_runtime();
    let parser = Arc::new(Mutex::new(StdinAnsiParser::new()));
    parser.lock().unwrap().open_forward(11);

    let fired: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));
    let fired_clone = fired.clone();
    schedule_timeout(
        rt.handle(),
        parser.clone(),
        ForwardKind::Barrier,
        11,
        Duration::from_millis(500),
        move |_, _| {
            *fired_clone.lock().unwrap() = true;
        },
    );

    // Before the deadline, simulate the barrier arriving: feed a reply
    // and the Primary-DA barrier; the parser's `completed_forward`
    // path clears the slot.
    {
        let mut p = parser.lock().unwrap();
        let mut chunk = Vec::new();
        chunk.extend_from_slice(b"\x1b]11;rgb:0/0/0\x1b\\");
        chunk.extend_from_slice(b"\x1b[c");
        let out = p.feed(&chunk);
        assert!(out.completed_forward.is_some(), "barrier should close slot");
    }

    // Now let the timer wake up — token-guard idempotency should make
    // it a no-op.
    rt.block_on(async {
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(600)).await;
        tokio::task::yield_now().await;
    });

    assert!(
        !*fired.lock().unwrap(),
        "timer must not invoke on_timeout once the barrier has closed the slot"
    );
}

#[test]
fn timer_is_noop_when_slot_holds_a_different_token() {
    // This models re-entry *after* the server has moved on: the old
    // timer's token no longer matches the active slot, so
    // `close_forward_on_timeout(old_token)` returns None and the
    // callback doesn't fire.
    let rt = paused_runtime();
    let parser = Arc::new(Mutex::new(StdinAnsiParser::new()));
    parser.lock().unwrap().open_forward(1);

    let fired: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));
    let fired_clone = fired.clone();
    schedule_timeout(
        rt.handle(),
        parser.clone(),
        ForwardKind::Barrier,
        1,
        Duration::from_millis(500),
        move |_, _| {
            *fired_clone.lock().unwrap() = true;
        },
    );

    // Close slot for token 1 via the barrier path, then open a fresh
    // slot for a different token. The earlier spawned timer is still
    // sleeping and holds a snapshot of token=1.
    {
        let mut p = parser.lock().unwrap();
        let mut chunk = Vec::new();
        chunk.extend_from_slice(b"\x1b[c");
        let _ = p.feed(&chunk); // close via barrier
        p.open_forward(2);
    }

    rt.block_on(async {
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(600)).await;
        tokio::task::yield_now().await;
    });

    assert!(
        !*fired.lock().unwrap(),
        "stale timer must not close the new slot"
    );
    assert_eq!(
        parser.lock().unwrap().active_forward_token(),
        Some(2),
        "new slot untouched"
    );
}

#[test]
fn timer_preserves_accumulated_reply_bytes_on_timeout() {
    // If the host went silent after emitting partial replies, the
    // timer still has to flush whatever landed in the buffer so the
    // pane sees *something* — empty is fine, partial is better.
    let rt = paused_runtime();
    let parser = Arc::new(Mutex::new(StdinAnsiParser::new()));
    parser.lock().unwrap().open_forward(22);

    // Simulate a single OSC 11 reply arriving before the host goes
    // silent.
    {
        let mut p = parser.lock().unwrap();
        let _ = p.feed(b"\x1b]11;rgb:1234/5678/9abc\x1b\\");
    }

    let captured: CapturedForward = Arc::new(Mutex::new(None));
    let captured_clone = captured.clone();
    schedule_timeout(
        rt.handle(),
        parser.clone(),
        ForwardKind::Barrier,
        22,
        Duration::from_millis(500),
        move |t, b| {
            *captured_clone.lock().unwrap() = Some((t, b));
        },
    );

    rt.block_on(async {
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(600)).await;
        tokio::task::yield_now().await;
    });

    let (token, bytes) = captured.lock().unwrap().take().expect("timer must fire");
    assert_eq!(token, 22);
    assert!(
        bytes.windows(4).any(|w| w == b"]11;"),
        "buffered OSC 11 bytes must appear in the flushed payload: {:?}",
        bytes
    );
}

#[test]
fn host_theme_dsr_997_dark_in_one_chunk() {
    let mut parser = StdinAnsiParser::new();
    let (replies, residue) = feed_once(&mut parser, b"\x1b[?997;1n");
    assert!(residue.is_empty(), "DSR 997 reply must be fully consumed");
    assert_eq!(replies.len(), 1);
    match &replies[0] {
        HostReply::HostTerminalThemeChanged(mode) => {
            assert_eq!(*mode, yinz_types::style::HostTerminalThemeMode::Dark);
        },
        other => panic!("expected HostTerminalThemeChanged, got {:?}", other),
    }
}

#[test]
fn host_theme_dsr_997_light_in_one_chunk() {
    let mut parser = StdinAnsiParser::new();
    let (replies, residue) = feed_once(&mut parser, b"\x1b[?997;2n");
    assert!(residue.is_empty());
    match &replies[0] {
        HostReply::HostTerminalThemeChanged(mode) => {
            assert_eq!(*mode, yinz_types::style::HostTerminalThemeMode::Light);
        },
        other => panic!("expected HostTerminalThemeChanged, got {:?}", other),
    }
}

#[test]
fn host_theme_dsr_997_across_chunk_boundaries() {
    // Same coverage philosophy as fragmented_csi_report_does_not_leak:
    // split the DSR at every internal byte position and assert the
    // parser still emits exactly one HostReply with no leaked residue.
    let full = b"\x1b[?997;1n";
    for split in 1..full.len() {
        let mut p = StdinAnsiParser::new();
        let r1 = p.feed(&full[..split]);
        let r2 = p.feed(&full[split..]);
        assert!(
            r1.residue.is_empty(),
            "split at {}: chunk 1 leaked residue {:?}",
            split,
            r1.residue
        );
        assert!(
            r2.residue.is_empty(),
            "split at {}: chunk 2 leaked residue {:?}",
            split,
            r2.residue
        );
        let total = r1.replies.len() + r2.replies.len();
        assert_eq!(total, 1, "split at {}: got {} replies", split, total);
        let reply = r1.replies.into_iter().chain(r2.replies).next().unwrap();
        match reply {
            HostReply::HostTerminalThemeChanged(mode) => {
                assert_eq!(mode, yinz_types::style::HostTerminalThemeMode::Dark);
            },
            other => panic!("expected HostTerminalThemeChanged, got {:?}", other),
        }
    }
}

#[test]
fn host_theme_dsr_997_unknown_param_dropped() {
    let mut parser = StdinAnsiParser::new();
    // CSI ?997;3n is not a defined Dark/Light value; the parser
    // recognises the CSI shape (final byte n) but
    // `from_csi_report` rejects unknown payloads, so no HostReply is
    // produced and no residue leaks (the bytes were consumed by termwiz).
    let out = parser.feed(b"\x1b[?997;3n");
    assert!(
        out.replies.is_empty(),
        "unknown ?997;N value must not classify"
    );
}

// =====================================================================
// Regression test: a Kitty keyboard-protocol event arriving on the
// client's stdin must not wedge the parser. Before the fix, such an
// event (`\x1b[<keycode>;<mods>u`) sat at the head of termwiz's
// internal buffer because parse_csi_report rejects the unwhitelisted
// final byte 'u' AND the keymap returned `Found::NeedData` (it's a
// possible prefix of registered key sequences). Every host reply that
// arrived behind it was silently swallowed, stalling host-color
// forwards until session timeout. This was confirmed via WEDGE-CANDIDATE
// log entries from three independent users on Kitty, Ghostty, and
// Konsole — all triggered by the same `\x1b[<digits>;<digits>u` shape.
//
// Captured triggers from the upstream Zellij repro logs (zellij-org/zellij#5149):
//   `\x1b[57414;129u`  (Kitty user, repro-logs/third-commit/zellij(4).log)
//   `\x1b[27;129u`     (Ghostty user, ...zellij(5).log)
//   `\x1b[102;133u`    (Konsole user, ...zellij_0.45_new-2.log)
// =====================================================================

#[test]
fn unsupported_or_malformed_input_does_not_wedge_subsequent_forward_reply() {
    let preludes: &[&[u8]] = &[
        b"\x1b[57414;129u", // captured from Kitty user
        b"\x1b[27;129u",    // captured from Ghostty user
        b"\x1b[102;133u",   // captured from Konsole user
        b"\x1b[A",          // legacy CSI key
        b"\x1b[?1u",
        b"\x1b[?6;1R",
        b"\x1b[<u",
        b"\x1b[<0;1M",
        b"\x1b[<0;;1m",
        b"\x1b[<999;1;1M",
        b"\x1b]11;interrupted\x1b[?1u",
        b"\x1b]11;interrupted",
        b"\x1b[?1\x01",
    ];
    let color = b"\x1b]11;rgb:1e1e/1e1e/2e2e\x1b\\";
    let barrier = b"\x1b[?62;52;c";

    for prelude in preludes {
        let expected_residue = [*prelude, b"z"].concat();
        let mut parser = StdinAnsiParser::new();
        parser.open_forward(42);
        let out = parser.feed(&[*prelude, b"z", color, barrier].concat());
        assert_eq!(
            out.completed_forward,
            Some((42, color.to_vec())),
            "{prelude:?}"
        );
        assert_eq!(out.residue, expected_residue);
        assert_eq!(parser.active_forward_token(), None);

        for split in 0..=prelude.len() {
            let mut parser = StdinAnsiParser::new();
            let mut residue = parser.feed(&prelude[..split]).residue;
            residue.extend(parser.feed(&prelude[split..]).residue);
            parser.open_forward(42);
            residue.extend(parser.feed(&[b"z", color.as_slice()].concat()).residue);
            let out = parser.feed(barrier);
            residue.extend(out.residue);
            assert_eq!(
                out.completed_forward,
                Some((42, color.to_vec())),
                "prelude {prelude:?}, split {split}",
            );
            assert_eq!(residue, expected_residue);
            assert_eq!(parser.active_forward_token(), None);
        }
    }
}

use crate::keyboard_parser::{KittyKeyboardParser, KittyParseOutcome};
use yinz_key_codec::vendored::termwiz::input::{
    InputEvent, InputParser, KeyCode, KeyEvent, Modifiers,
};
use yinz_types::keys::BareKey;

fn assert_held_across_idle(parser: &mut StdinAnsiParser) {
    assert_eq!(
        parser.pending_partial(),
        PendingPartial::ReplyInProgress,
        "a payload-bearing partial must report ReplyInProgress while held"
    );
    assert!(
        parser.finalize_fast_partial().is_empty(),
        "the short idle finalize must NOT drain a reply-in-progress partial"
    );
    assert_eq!(
        parser.pending_partial(),
        PendingPartial::ReplyInProgress,
        "the partial must still be held after a lone-esc finalize"
    );
}

#[test]
fn fragmented_osc4_reply_is_retained_not_leaked() {
    let mut p = StdinAnsiParser::new();
    let r1 = p.feed(b"\x1b]4;1;rgb:1111");
    assert!(
        r1.residue.is_empty(),
        "incomplete OSC 4 must not leak: {:?}",
        r1.residue
    );
    assert!(r1.replies.is_empty());
    assert_held_across_idle(&mut p);

    let r2 = p.feed(b"/2222/3333\x1b\\");
    assert!(
        r2.residue.is_empty(),
        "completion must not leak: {:?}",
        r2.residue
    );
    assert_eq!(r2.replies.len(), 1, "the rejoined OSC 4 classifies once");
    match &r2.replies[0] {
        HostReply::ColorRegisters(regs) => {
            assert_eq!(regs[0].0, 1);
            assert_eq!(regs[0].1, "rgb:1111/2222/3333");
        },
        other => panic!("expected ColorRegisters, got {:?}", other),
    }
    assert_eq!(p.pending_partial(), PendingPartial::None);
}

#[test]
fn fragmented_osc11_reply_is_retained_across_idle() {
    let mut p = StdinAnsiParser::new();
    let r1 = p.feed(b"\x1b]11;rgb:0000/00");
    assert!(r1.residue.is_empty());
    assert_held_across_idle(&mut p);

    let r2 = p.feed(b"00/0000\x1b\\");
    assert!(r2.residue.is_empty());
    assert_eq!(r2.replies.len(), 1);
    matches!(r2.replies[0], HostReply::BackgroundColor(_));
}

#[test]
fn esc_companion_chord_is_not_held_as_reply() {
    let mut whole = StdinAnsiParser::new();
    let out = whole.feed(b"\x1ba");
    assert_eq!(out.residue, b"\x1ba");
    assert!(out.replies.is_empty());

    let mut split = StdinAnsiParser::new();
    let r1 = split.feed(b"\x1b");
    assert!(
        r1.residue.is_empty(),
        "lone ESC must park until disambiguated"
    );
    assert_eq!(split.pending_partial(), PendingPartial::LoneEsc);
    let r2 = split.feed(b"a");
    let mut combined = r1.residue.clone();
    combined.extend_from_slice(&r2.residue);
    assert_eq!(combined, b"\x1ba");
    assert!(r2.replies.is_empty());
}

#[test]
fn fragmented_ctrl_arrow_is_retained_then_passed_through() {
    let mut p = StdinAnsiParser::new();
    let r1 = p.feed(b"\x1b[1;5");
    assert!(r1.residue.is_empty(), "incomplete CSI must not leak");
    assert_held_across_idle(&mut p);

    let r2 = p.feed(b"C");
    assert_eq!(r2.residue, b"\x1b[1;5C", "the arrow reassembles intact");
    assert!(r2.replies.is_empty(), "a cursor key is never a host reply");
}

#[test]
fn fragmented_sgr_mouse_is_retained_then_passed_through() {
    let mut p = StdinAnsiParser::new();
    let r1 = p.feed(b"\x1b[<0;12;3");
    assert!(r1.residue.is_empty());
    assert_held_across_idle(&mut p);
    let r2 = p.feed(b"4M");
    assert_eq!(r2.residue, b"\x1b[<0;12;34M");
    assert!(r2.replies.is_empty());
}

#[test]
fn focus_event_is_residue_not_reply() {
    let mut whole = StdinAnsiParser::new();
    let out = whole.feed(b"\x1b[I");
    assert_eq!(out.residue, b"\x1b[I");
    assert!(out.replies.is_empty());

    let mut split = StdinAnsiParser::new();
    let r1 = split.feed(b"\x1b[");
    assert!(r1.residue.is_empty());
    let r2 = split.feed(b"I");
    let mut combined = r1.residue.clone();
    combined.extend_from_slice(&r2.residue);
    assert_eq!(combined, b"\x1b[I");
    assert!(r2.replies.is_empty());
}

#[test]
fn kitty_keyboard_variants_whole_and_fragmented() {
    let variants: Vec<&[u8]> = vec![
        b"\x1b[97u",
        b"\x1b[97;5u",
        b"\x1b[97;1:3u",
        b"\x1b[97:65;5u",
        b"\x1b[97;5;65u",
        b"\x1b[1;5A",
        b"\x1b[1;2H",
        b"\x1b[1;2P",
        b"\x1b[1;2S",
        b"\x1b[2;5~",
        b"\x1b[3;5~",
        b"\x1b[15;2~",
        b"\x1b[27u",
        b"\x1b[57362u",
        b"\x1b[Z",
    ];
    for seq in variants {
        let pretty = String::from_utf8_lossy(seq).replace('\x1b', "ESC");

        let mut whole = StdinAnsiParser::new();
        let out = whole.feed(seq);
        assert_eq!(
            out.residue, seq,
            "{}: whole residue must equal the sequence",
            pretty
        );
        assert!(out.replies.is_empty(), "{}: a key is never a reply", pretty);

        for split in 2..seq.len() {
            let mut p = StdinAnsiParser::new();
            let r1 = p.feed(&seq[..split]);
            assert!(
                r1.residue.is_empty(),
                "{} split at {}: prefix must be held, not leaked: {:?}",
                pretty,
                split,
                r1.residue
            );
            let expected_class = if split == 2 {
                PendingPartial::BareIntroducer
            } else {
                PendingPartial::ReplyInProgress
            };
            assert_eq!(
                p.pending_partial(),
                expected_class,
                "{} split at {}: unexpected partial classification",
                pretty,
                split
            );
            if split > 2 {
                assert!(
                    p.finalize_fast_partial().is_empty(),
                    "{} split at {}: idle must not drain the held key",
                    pretty,
                    split
                );
            }
            let r2 = p.feed(&seq[split..]);
            let mut combined = r1.residue.clone();
            combined.extend_from_slice(&r2.residue);
            assert_eq!(
                combined, seq,
                "{} split at {}: reassembled residue must equal the sequence",
                pretty, split
            );
            assert!(
                r1.replies.is_empty() && r2.replies.is_empty(),
                "{} split at {}: never classified as a reply",
                pretty,
                split
            );
        }
    }
}

fn keys_from_flushed_bytes(bytes: &[u8]) -> Vec<InputEvent> {
    let mut parser = InputParser::new();
    parser.parse_as_vec(bytes, false)
}

fn alt_char(c: char) -> InputEvent {
    InputEvent::Key(KeyEvent {
        key: KeyCode::Char(c),
        modifiers: Modifiers::ALT,
    })
}

#[test]
fn a_bare_osc_introducer_is_fast_flushed_as_alt_close_bracket() {
    let mut p = StdinAnsiParser::new();
    let out = p.feed(b"\x1b]");
    assert!(
        out.residue.is_empty(),
        "the introducer parks until disambiguated: {:?}",
        out.residue
    );
    assert_eq!(
        p.pending_partial(),
        PendingPartial::BareIntroducer,
        "a body-less ESC ] belongs to the fast-flush class"
    );
    assert!(
        !p.pending_partial().uses_reply_flush_guard(),
        "a bare introducer must not wait behind the long reply guard"
    );
    let drained = p.finalize_fast_partial();
    assert_eq!(
        drained,
        b"\x1b]".to_vec(),
        "the fast finalize must release the introducer"
    );
    assert_eq!(
        p.pending_partial(),
        PendingPartial::None,
        "the parser holds nothing after the fast flush"
    );
    assert_eq!(
        keys_from_flushed_bytes(&drained),
        vec![alt_char(']')],
        "the flushed bytes must decode as Alt+], not garbage"
    );
}

#[test]
fn a_bare_csi_introducer_is_fast_flushed_as_alt_open_bracket() {
    let mut p = StdinAnsiParser::new();
    let out = p.feed(b"\x1b[");
    assert!(
        out.residue.is_empty(),
        "the introducer parks until disambiguated: {:?}",
        out.residue
    );
    assert_eq!(
        p.pending_partial(),
        PendingPartial::BareIntroducer,
        "a body-less ESC [ belongs to the fast-flush class"
    );
    assert!(!p.pending_partial().uses_reply_flush_guard());
    let drained = p.finalize_fast_partial();
    assert_eq!(drained, b"\x1b[".to_vec());
    assert_eq!(p.pending_partial(), PendingPartial::None);
    assert_eq!(
        keys_from_flushed_bytes(&drained),
        vec![alt_char('[')],
        "the flushed bytes must decode as Alt+[, not garbage"
    );
}

#[test]
fn an_introducer_with_a_body_byte_keeps_the_long_reply_guard() {
    for prefix in [b"\x1b]4".as_ref(), b"\x1b[?".as_ref()] {
        let pretty = String::from_utf8_lossy(prefix).replace('\x1b', "ESC");
        let mut p = StdinAnsiParser::new();
        let out = p.feed(prefix);
        assert!(out.residue.is_empty(), "{}: must not leak", pretty);
        assert_eq!(
            p.pending_partial(),
            PendingPartial::ReplyInProgress,
            "{}: one body byte crosses the tier boundary",
            pretty
        );
        assert!(p.pending_partial().uses_reply_flush_guard());
        assert!(
            p.finalize_fast_partial().is_empty(),
            "{}: the fast flush must not touch a payload-bearing partial",
            pretty
        );
        assert_eq!(p.pending_partial(), PendingPartial::ReplyInProgress);
    }
}

#[test]
fn the_flush_path_is_selected_by_classification_not_by_wall_clock() {
    let cases: Vec<(&[u8], PendingPartial, bool)> = vec![
        (b"\x1b", PendingPartial::LoneEsc, false),
        (b"\x1b[", PendingPartial::BareIntroducer, false),
        (b"\x1b]", PendingPartial::BareIntroducer, false),
        (b"\x1b[1;5", PendingPartial::ReplyInProgress, true),
        (b"\x1b]4;1;rg", PendingPartial::ReplyInProgress, true),
    ];
    for (bytes, expected_class, guarded) in cases {
        let pretty = String::from_utf8_lossy(bytes).replace('\x1b', "ESC");
        let mut p = StdinAnsiParser::new();
        let _ = p.feed(bytes);
        assert_eq!(
            p.pending_partial(),
            expected_class,
            "{}: unexpected classification",
            pretty
        );
        assert_eq!(
            p.pending_partial().uses_reply_flush_guard(),
            guarded,
            "{}: the guard tier is decided by the classification alone",
            pretty
        );
        assert_eq!(
            p.finalize_fast_partial().is_empty(),
            guarded,
            "{}: the fast finalize drains exactly the unguarded classes",
            pretty
        );
    }
}

#[test]
fn an_osc_reply_fragmented_at_exactly_the_introducer_is_still_stripped() {
    let mut p = StdinAnsiParser::new();
    let r1 = p.feed(b"\x1b]");
    assert!(r1.residue.is_empty(), "the introducer must not leak");
    assert_eq!(p.pending_partial(), PendingPartial::BareIntroducer);

    let r2 = p.feed(b"4;1;rgb:1111/2222/3333\x1b\\");
    assert!(
        r2.residue.is_empty(),
        "a reply rejoined before any idle finalize must never leak as keys: {:?}",
        r2.residue
    );
    assert_eq!(r2.replies.len(), 1, "the rejoined OSC 4 classifies once");
    match &r2.replies[0] {
        HostReply::ColorRegisters(regs) => {
            assert_eq!(regs[0].0, 1);
            assert_eq!(regs[0].1, "rgb:1111/2222/3333");
        },
        other => panic!("expected ColorRegisters, got {:?}", other),
    }
    assert_eq!(p.pending_partial(), PendingPartial::None);
}

#[test]
fn a_csi_report_fragmented_at_exactly_the_introducer_is_still_stripped() {
    let mut p = StdinAnsiParser::new();
    let r1 = p.feed(b"\x1b[");
    assert!(r1.residue.is_empty());
    assert_eq!(p.pending_partial(), PendingPartial::BareIntroducer);

    let r2 = p.feed(b"?2026;1$y");
    assert!(
        r2.residue.is_empty(),
        "a DECRPM rejoined before any idle finalize must not leak: {:?}",
        r2.residue
    );
    assert_eq!(r2.replies.len(), 1);
    match &r2.replies[0] {
        HostReply::SynchronizedOutput(Some(_)) => {},
        other => panic!("unexpected reply: {:?}", other),
    }
    assert_eq!(p.pending_partial(), PendingPartial::None);
}

#[test]
fn kitty_ctrl_a_trace_yields_one_event_not_a_literal_u() {
    let mut p = StdinAnsiParser::new();
    let r1 = p.feed(b"\x1b[97;5");
    assert!(r1.residue.is_empty());
    assert_held_across_idle(&mut p);
    let r2 = p.feed(b"u");
    assert_eq!(r2.residue, b"\x1b[97;5u");

    let mut kitty = KittyKeyboardParser::new();
    match kitty.feed(&r2.residue) {
        KittyParseOutcome::Complete(key, _) => {
            assert!(
                key.is_key_with_ctrl_modifier(BareKey::Char('a')),
                "expected Ctrl-a, got {:?}",
                key
            );
        },
        other => panic!("expected a complete Ctrl-a, got {:?}", other),
    }
}

#[test]
fn bracketed_paste_preserves_embedded_escape_content() {
    let mut p = StdinAnsiParser::new();
    let mut chunk = Vec::new();
    chunk.extend_from_slice(b"\x1b[200~");
    chunk.extend_from_slice(b"echo \x1b]0;hi\x1b\\ done");
    chunk.extend_from_slice(b"\x1b[201~");
    let out = p.feed(&chunk);
    assert!(
        out.replies.is_empty(),
        "paste content must not classify as a reply: {:?}",
        out.replies
    );
    assert!(
        out.desktop_notifications.is_empty(),
        "an OSC inside a paste is literal, not a notification"
    );
    assert_eq!(
        out.residue, chunk,
        "the entire paste body, including the embedded OSC, passes through verbatim"
    );
    assert!(
        out.residue.windows(6).any(|w| w == b"\x1b]0;hi"),
        "the embedded OSC bytes must survive in residue"
    );
}

#[test]
fn paste_embedded_osc_is_not_eaten_when_fragmented() {
    let full: Vec<u8> = b"\x1b[200~A\x1b]11;rgb:0/0/0\x1b\\B\x1b[201~".to_vec();
    for split in 1..full.len() {
        let mut p = StdinAnsiParser::new();
        let r1 = p.feed(&full[..split]);
        let r2 = p.feed(&full[split..]);
        let mut combined = r1.residue.clone();
        combined.extend_from_slice(&r2.residue);
        assert_eq!(
            combined, full,
            "split at {}: paste body (with embedded OSC) must pass through whole",
            split
        );
        assert!(
            r1.replies.is_empty() && r2.replies.is_empty(),
            "split at {}: embedded OSC must not classify as a reply",
            split
        );
    }
}

#[test]
fn paste_markers_fragmented_still_toggle_paste_mode() {
    {
        let (head, tail) = (b"\x1b[20".as_ref(), b"0~".as_ref());
        let mut p = StdinAnsiParser::new();
        let r1 = p.feed(head);
        assert!(r1.residue.is_empty(), "fragmented start marker is held");
        let r2 = p.feed(tail);
        let mut start = r1.residue.clone();
        start.extend_from_slice(&r2.residue);
        assert_eq!(
            start, b"\x1b[200~",
            "start marker passes through to residue"
        );

        let body = p.feed(b"\x1b]0;x\x1b\\");
        assert_eq!(body.residue, b"\x1b]0;x\x1b\\");
        assert!(body.replies.is_empty());
    }
    {
        let (head, tail) = (b"\x1b[20".as_ref(), b"1~".as_ref());
        let mut p = StdinAnsiParser::new();
        let _ = p.feed(b"\x1b[200~body");
        let r1 = p.feed(head);
        let r2 = p.feed(tail);
        let mut end = r1.residue.clone();
        end.extend_from_slice(&r2.residue);
        assert_eq!(end, b"\x1b[201~", "end marker passes through to residue");

        let after = p.feed(b"\x1b]11;rgb:1/2/3\x1b\\");
        assert!(after.residue.is_empty());
        assert_eq!(after.replies.len(), 1);
    }
}

#[test]
fn last_resort_force_drain_recovers_a_never_terminated_partial() {
    let mut p = StdinAnsiParser::new();
    let r1 = p.feed(b"\x1b]4;1;rgb:incomplete");
    assert!(r1.residue.is_empty());
    assert_eq!(p.pending_partial(), PendingPartial::ReplyInProgress);

    let r2 = p.feed(b"and-more");
    assert!(r2.residue.is_empty());
    assert_eq!(p.pending_partial(), PendingPartial::ReplyInProgress);

    let drained = p.finalize_force();
    assert!(
        drained.windows(14).any(|w| w == b"rgb:incomplete"),
        "force-drain must recover the buffered partial: {:?}",
        drained
    );
    assert!(
        drained.windows(8).any(|w| w == b"and-more"),
        "force-drain must recover bytes appended before the guard: {:?}",
        drained
    );
    assert_eq!(
        p.pending_partial(),
        PendingPartial::None,
        "the buffer is empty after a force-drain"
    );
}

#[test]
fn kitty_probe_ok_reply_classifies_true() {
    let mut parser = StdinAnsiParser::new();
    parser.expect_kitty_probe_reply();
    let (replies, residue) = feed_once(&mut parser, b"\x1b_Gi=31;OK\x1b\\");
    assert!(residue.is_empty(), "probe reply must be fully consumed");
    assert_eq!(replies.len(), 1);
    match &replies[0] {
        HostReply::KittyGraphicsSupport(supported) => assert!(*supported),
        other => panic!("unexpected reply: {:?}", other),
    }
    let duplicate = parser.feed(b"\x1b_Gi=31;OK\x1b\\");
    assert!(duplicate.replies.is_empty());
    assert!(duplicate.residue.is_empty());
    assert!(parser.resolve_kitty_probe_by_timeout().is_none());
}

#[test]
fn kitty_probe_error_reply_classifies_false() {
    let mut parser = StdinAnsiParser::new();
    parser.expect_kitty_probe_reply();
    let (replies, residue) = feed_once(&mut parser, b"\x1b_Gi=31;ENOTSUPPORTED:query failed\x1b\\");
    assert!(residue.is_empty());
    assert_eq!(replies.len(), 1);
    match &replies[0] {
        HostReply::KittyGraphicsSupport(supported) => assert!(!*supported),
        other => panic!("unexpected reply: {:?}", other),
    }
    let late_ok = parser.feed(b"\x1b_Gi=31;OK\x1b\\");
    assert!(late_ok.replies.is_empty());
    assert!(late_ok.residue.is_empty());
    assert!(parser.resolve_kitty_probe_by_timeout().is_none());
}

/// The deadline exists for a transport that answers neither the probe nor the barrier that
/// backstops it - mosh's emulator swallows the APC, and a proxy may swallow both - which would
/// otherwise leave the capability unknown for the life of the session.
#[test]
fn a_probe_that_is_never_answered_resolves_false_on_the_deadline() {
    let mut parser = StdinAnsiParser::new();
    parser.expect_kitty_probe_reply();
    match parser.resolve_kitty_probe_by_timeout() {
        Some(HostReply::KittyGraphicsSupport(supported)) => assert!(!supported),
        other => panic!("unexpected reply: {:?}", other),
    }
}

#[test]
fn the_probe_deadline_only_reports_once() {
    let mut parser = StdinAnsiParser::new();
    parser.expect_kitty_probe_reply();
    assert!(parser.resolve_kitty_probe_by_timeout().is_some());
    assert!(parser.resolve_kitty_probe_by_timeout().is_none());
}

#[test]
fn the_probe_deadline_is_inert_before_the_probe_is_armed() {
    let mut parser = StdinAnsiParser::new();
    assert!(parser.resolve_kitty_probe_by_timeout().is_none());
}

/// A slow host is not an incapable one: the deadline reports "no graphics" to get a working
/// answer out, but a reply arriving afterwards still turns the capability on.
#[test]
fn a_late_ok_after_the_deadline_still_reports_support() {
    let mut parser = StdinAnsiParser::new();
    parser.expect_kitty_probe_reply();
    assert!(parser.resolve_kitty_probe_by_timeout().is_some());
    let (replies, residue) = feed_once(&mut parser, b"\x1b_Gi=31;OK\x1b\\");
    assert!(residue.is_empty(), "probe reply must be fully consumed");
    let kitty_replies: Vec<bool> = replies
        .iter()
        .filter_map(|r| match r {
            HostReply::KittyGraphicsSupport(supported) => Some(*supported),
            _ => None,
        })
        .collect();
    assert_eq!(kitty_replies, vec![true]);
}

/// Saying `false` twice would cost the server a whole `ModeUpdate` fan-out to every plugin for a
/// value that did not change.
#[test]
fn a_barrier_after_the_deadline_keeps_waiting_without_repeating_the_answer() {
    let mut parser = StdinAnsiParser::new();
    parser.expect_kitty_probe_reply();
    assert!(parser.resolve_kitty_probe_by_timeout().is_some());
    let (replies, residue) = feed_once(&mut parser, b"\x1b[?62;4;52c");
    assert!(residue.is_empty());
    assert!(
        !replies
            .iter()
            .any(|r| matches!(r, HostReply::KittyGraphicsSupport(_))),
        "{replies:?}"
    );
    // The barrier's other job is unaffected.
    assert!(replies
        .iter()
        .any(|r| matches!(r, HostReply::SixelSupport(true))));
    assert!(parser.resolve_kitty_probe_by_timeout().is_none());
    let late_ok = parser.feed(b"\x1b_Gi=31;OK\x1b\\");
    assert!(late_ok.residue.is_empty());
    assert!(matches!(
        late_ok.replies.as_slice(),
        [HostReply::KittyGraphicsSupport(true)]
    ));
}

/// A provisional negative already said `false`; a refusal settles the probe without repeating it.
#[test]
fn a_late_refusal_settles_the_probe_without_repeating_the_answer() {
    for timeout_first in [false, true] {
        let mut parser = StdinAnsiParser::new();
        parser.expect_kitty_probe_reply();
        if timeout_first {
            assert!(parser.resolve_kitty_probe_by_timeout().is_some());
        } else {
            let _ = parser.feed(b"\x1b[?62;4;52c");
        }
        let refusal = parser.feed(b"\x1b_Gi=31;ENOENT:no such file\x1b\\");
        assert!(refusal.residue.is_empty());
        assert!(refusal.replies.is_empty());
        let late_ok = parser.feed(b"\x1b_Gi=31;OK\x1b\\");
        assert!(late_ok.residue.is_empty());
        assert!(late_ok.replies.is_empty());
        assert!(parser.resolve_kitty_probe_by_timeout().is_none());
    }
}

#[test]
fn kitty_probe_absence_reports_false_only_on_the_first_barrier() {
    let mut parser = StdinAnsiParser::new();
    parser.expect_kitty_probe_reply();
    let (replies, residue) = feed_once(&mut parser, b"\x1b[?62;4;52c");
    assert!(residue.is_empty());
    let kitty_replies: Vec<bool> = replies
        .iter()
        .filter_map(|r| match r {
            HostReply::KittyGraphicsSupport(supported) => Some(*supported),
            _ => None,
        })
        .collect();
    assert_eq!(kitty_replies, vec![false]);
    for _ in 0..2 {
        let out = parser.feed(b"\x1b[?62;4;52c");
        assert!(out.residue.is_empty());
        assert!(!out
            .replies
            .iter()
            .any(|r| matches!(r, HostReply::KittyGraphicsSupport(_))));
        assert!(parser.resolve_kitty_probe_by_timeout().is_none());
    }
    let late_ok = parser.feed(b"\x1b_Gi=31;OK\x1b\\");
    assert!(late_ok.residue.is_empty());
    assert!(matches!(
        late_ok.replies.as_slice(),
        [HostReply::KittyGraphicsSupport(true)]
    ));
}

#[test]
fn kitty_probe_barrier_then_success_is_independent_of_read_boundaries() {
    let full = b"\x1b[?62;4;52c\x1b_Gi=31;OK\x1b\\";
    // Includes both replies in one read, every split, and one byte per read.
    let chunkings = (0..=full.len())
        .map(|split| vec![&full[..split], &full[split..]])
        .chain(std::iter::once(full.chunks(1).collect()));
    for chunks in chunkings {
        let mut parser = StdinAnsiParser::new();
        parser.expect_kitty_probe_reply();
        let mut support = Vec::new();
        for chunk in &chunks {
            let out = parser.feed(chunk);
            assert!(out.residue.is_empty(), "chunks: {chunks:?}, out: {out:?}");
            support.extend(out.replies.into_iter().filter_map(|reply| match reply {
                HostReply::KittyGraphicsSupport(supported) => Some(supported),
                _ => None,
            }));
        }
        // Extracting APCs first can skip the provisional negative in a shared read.
        assert!(
            support == [true] || support == [false, true],
            "chunks: {chunks:?}, support: {support:?}"
        );
        assert_eq!(parser.pending_partial(), PendingPartial::None);
        assert!(parser.finalize_force().is_empty());
        assert!(parser.resolve_kitty_probe_by_timeout().is_none());
    }
}

#[test]
fn primary_da_with_sixel_attribute_classifies_sixel_support_true() {
    let mut parser = StdinAnsiParser::new();
    let (replies, residue) = feed_once(&mut parser, b"\x1b[?62;4;52c");
    assert!(residue.is_empty());
    let sixel_replies: Vec<bool> = replies
        .iter()
        .filter_map(|r| match r {
            HostReply::SixelSupport(supported) => Some(*supported),
            _ => None,
        })
        .collect();
    assert_eq!(sixel_replies, vec![true]);
}

#[test]
fn primary_da_without_sixel_attribute_classifies_sixel_support_false() {
    let mut parser = StdinAnsiParser::new();
    let (replies, residue) = feed_once(&mut parser, b"\x1b[?62;22c");
    assert!(residue.is_empty());
    let sixel_replies: Vec<bool> = replies
        .iter()
        .filter_map(|r| match r {
            HostReply::SixelSupport(supported) => Some(*supported),
            _ => None,
        })
        .collect();
    assert_eq!(sixel_replies, vec![false]);
}

#[test]
fn primary_da_sixel_attribute_is_not_matched_as_a_substring() {
    let mut parser = StdinAnsiParser::new();
    let (replies, residue) = feed_once(&mut parser, b"\x1b[?64;14;21;22c");
    assert!(residue.is_empty());
    let sixel_replies: Vec<bool> = replies
        .iter()
        .filter_map(|r| match r {
            HostReply::SixelSupport(supported) => Some(*supported),
            _ => None,
        })
        .collect();
    assert_eq!(sixel_replies, vec![false]);
}

#[test]
fn kitty_probe_reply_then_barrier_emits_exactly_one_reply() {
    let mut parser = StdinAnsiParser::new();
    parser.expect_kitty_probe_reply();
    let mut chunk = Vec::new();
    chunk.extend_from_slice(b"\x1b_Gi=31;OK\x1b\\");
    chunk.extend_from_slice(b"\x1b[?62;4;52c");
    let (replies, residue) = feed_once(&mut parser, &chunk);
    assert!(residue.is_empty());
    let kitty_replies: Vec<bool> = replies
        .iter()
        .filter_map(|r| match r {
            HostReply::KittyGraphicsSupport(supported) => Some(*supported),
            _ => None,
        })
        .collect();
    assert_eq!(kitty_replies, vec![true]);
}

#[test]
fn kitty_probe_reply_fragmented_across_feeds() {
    let full = b"\x1b_Gi=31;OK\x1b\\";
    for split in 1..full.len() {
        let mut parser = StdinAnsiParser::new();
        parser.expect_kitty_probe_reply();
        let r1 = parser.feed(&full[..split]);
        let r2 = parser.feed(&full[split..]);
        assert!(
            r1.residue.is_empty(),
            "split at {}: chunk 1 residue {:?}",
            split,
            r1.residue
        );
        assert!(
            r2.residue.is_empty(),
            "split at {}: chunk 2 residue {:?}",
            split,
            r2.residue
        );
        assert_eq!(
            r1.replies.len() + r2.replies.len(),
            1,
            "split at {}: exactly one reply across both chunks",
            split
        );
        let reply = r1.replies.into_iter().chain(r2.replies).next().unwrap();
        match reply {
            HostReply::KittyGraphicsSupport(supported) => assert!(supported),
            other => panic!("split at {}: unexpected reply {:?}", split, other),
        }
    }
}

#[test]
fn apc_outside_probe_window_is_not_classified() {
    let mut parser = StdinAnsiParser::new();
    let out = parser.feed(b"\x1b_Gi=31;OK\x1b\\");
    assert!(out.replies.is_empty());
    assert!(out.residue.is_empty());
}

#[test]
fn unsolicited_kitty_graphics_reply_never_becomes_keyboard_input() {
    let mut parser = StdinAnsiParser::new();
    parser.expect_kitty_probe_reply();
    let out = parser.feed(b"\x1b_Gi=31;OK\x1b\\");
    assert_eq!(out.replies.len(), 1);

    let out = parser.feed(b"\x1b_Gi=2000000000;OK\x1b\\");
    assert!(
        out.residue.is_empty(),
        "a host reply to our own image transmission must not be typed into the focused pane, got: {:?}",
        out.residue
    );
    assert!(out.replies.is_empty());

    let mut fragmented = StdinAnsiParser::new();
    fragmented.expect_kitty_probe_reply();
    let _ = fragmented.feed(b"\x1b_Gi=31;OK\x1b\\");
    let first = fragmented.feed(b"\x1b_Gi=2000000");
    let second = fragmented.feed(b"000;OK\x1b\\");
    assert!(first.residue.is_empty() && second.residue.is_empty());
}

#[test]
fn non_kitty_apc_sequences_are_left_alone() {
    let mut parser = StdinAnsiParser::new();
    parser.expect_kitty_probe_reply();
    let out = parser.feed(b"\x1b_Xsomething\x1b\\");
    assert!(
        !out.residue.is_empty(),
        "only kitty graphics APCs may be swallowed"
    );
}

#[test]
fn unsolicited_theme_notification_classifies_without_outstanding_query() {
    let mut p = StdinAnsiParser::new();
    let out = p.feed(b"\x1b[?997;1n");
    assert!(
        out.residue.is_empty(),
        "the notification must not leak as residue"
    );
    assert_eq!(out.replies.len(), 1);
    match &out.replies[0] {
        HostReply::HostTerminalThemeChanged(mode) => {
            assert_eq!(*mode, yinz_types::style::HostTerminalThemeMode::Dark);
        },
        other => panic!("expected HostTerminalThemeChanged, got {:?}", other),
    }
}

fn announce_frame_and_payload() -> (Vec<u8>, Vec<u8>) {
    use {
        yinz_ipc::nested_session::encode_frame, yinz_ipc::nested_session::encode_payload,
        yinz_ipc::nested_session::NestedSessionCapability,
        yinz_ipc::nested_session::NestedSessionMessage,
    };
    let message = NestedSessionMessage::Announce {
        session_name: "guest-session".to_owned().into(),
        capabilities: vec![NestedSessionCapability::NestedControl],
    };
    (encode_frame(&message), encode_payload(&message))
}

#[test]
fn nested_frame_in_one_chunk_is_extracted_with_no_residue() {
    let (frame, payload) = announce_frame_and_payload();
    let mut p = StdinAnsiParser::new();
    let out = p.feed(&frame);
    assert_eq!(out.nested_frames, vec![payload]);
    assert!(out.residue.is_empty(), "residue: {:?}", out.residue);
    assert_eq!(p.pending_partial(), PendingPartial::None);
}

#[test]
fn nested_frame_split_at_every_boundary_is_extracted() {
    let (frame, payload) = announce_frame_and_payload();
    for split_at in 1..frame.len() {
        let mut p = StdinAnsiParser::new();
        let first = p.feed(&frame[..split_at]);
        let second = p.feed(&frame[split_at..]);
        let mut extracted = first.nested_frames.clone();
        extracted.extend(second.nested_frames.clone());
        assert_eq!(extracted, vec![payload.clone()], "split at {}", split_at);
        assert!(
            first.residue.is_empty() && second.residue.is_empty(),
            "split at {}: residue {:?} / {:?}",
            split_at,
            first.residue,
            second.residue
        );
    }
}

#[test]
fn keystrokes_around_nested_frame_survive_as_residue() {
    let (frame, payload) = announce_frame_and_payload();
    let mut chunk = b"abc".to_vec();
    chunk.extend_from_slice(&frame);
    chunk.extend_from_slice(b"def");
    let mut p = StdinAnsiParser::new();
    let out = p.feed(&chunk);
    assert_eq!(out.nested_frames, vec![payload]);
    assert_eq!(out.residue, b"abcdef".to_vec());
}

#[test]
fn nested_frame_with_garbage_base64_is_stripped_without_payload() {
    let mut p = StdinAnsiParser::new();
    let out = p.feed(b"\x1bP26661n!!!not-base64!!!\x1b\\");
    assert!(out.nested_frames.is_empty());
    assert!(out.residue.is_empty(), "residue: {:?}", out.residue);
}

#[test]
fn foreign_dcs_passes_through_untouched() {
    let foreign_dcs = b"\x1bP1$qm\x1b\\q".to_vec();
    let mut p = StdinAnsiParser::new();
    let out = p.feed(&foreign_dcs);
    assert!(out.nested_frames.is_empty());
    assert_eq!(out.residue, foreign_dcs);
}

#[test]
fn lone_esc_claimed_by_frame_prepass_reroutes_on_next_chunk() {
    let mut p = StdinAnsiParser::new();
    let first = p.feed(b"\x1b");
    assert!(first.residue.is_empty());
    assert!(first.has_partial_state);
    assert_eq!(p.pending_partial(), PendingPartial::LoneEsc);
    let second = p.feed(b"abc");
    assert_eq!(second.residue, b"\x1babc".to_vec());
    assert_eq!(p.pending_partial(), PendingPartial::None);
}

#[test]
fn unterminated_frame_header_prefix_is_released_by_force_drain() {
    let mut p = StdinAnsiParser::new();
    let out = p.feed(b"\x1bP26");
    assert!(out.residue.is_empty());
    assert!(out.has_partial_state);
    assert_eq!(p.pending_partial(), PendingPartial::ReplyInProgress);
    assert_eq!(p.finalize_force(), b"\x1bP26".to_vec());
    assert_eq!(p.pending_partial(), PendingPartial::None);
}

#[test]
fn two_nested_frames_in_one_chunk_are_both_extracted() {
    let (frame, payload) = announce_frame_and_payload();
    let mut chunk = frame.clone();
    chunk.extend_from_slice(&frame);
    let mut p = StdinAnsiParser::new();
    let out = p.feed(&chunk);
    assert_eq!(out.nested_frames, vec![payload.clone(), payload]);
    assert!(out.residue.is_empty());
}

#[test]
fn clipboard_forward_is_closed_by_the_hosts_osc_52_reply() {
    let mut parser = StdinAnsiParser::new();
    parser.open_clipboard_forward(9);
    let out = parser.feed(b"\x1b]52;c;aGVsbG8=\x1b\\");
    assert_eq!(
        out.completed_clipboard_forward,
        Some((9, b"\x1b]52;c;aGVsbG8=\x1b\\".to_vec())),
        "the host's reply must be relayed verbatim (re-serialized with ST)"
    );
    assert!(out.residue.is_empty(), "clipboard data must never be typed");
    assert!(parser.active_clipboard_forward_token().is_none());
}

#[test]
fn clipboard_forward_is_closed_by_the_first_key_press() {
    let mut parser = StdinAnsiParser::new();
    parser.open_clipboard_forward(4);
    let out = parser.feed(b"a");
    assert_eq!(out.completed_clipboard_forward, Some((4, Vec::new())));
    assert_eq!(out.residue, b"a", "the key itself must still reach the app");
    assert!(parser.active_clipboard_forward_token().is_none());
}

#[test]
fn clipboard_forward_survives_host_reports_and_resizes() {
    let mut parser = StdinAnsiParser::new();
    parser.open_clipboard_forward(11);
    let out = parser.feed(b"\x1b[4;720;1280t");
    assert!(out.completed_clipboard_forward.is_none());
    let out = parser.feed(b"\x1b]11;rgb:0000/0000/0000\x1b\\");
    assert!(out.completed_clipboard_forward.is_none());
    let out = parser.feed(b"\x1b[?62;52c");
    assert!(
        out.completed_clipboard_forward.is_none(),
        "the Primary-DA barrier must not close a clipboard window"
    );
    assert_eq!(parser.active_clipboard_forward_token(), Some(11));
}

#[test]
fn a_reply_arriving_before_the_keypress_wins() {
    let mut parser = StdinAnsiParser::new();
    parser.open_clipboard_forward(2);
    let mut input = Vec::new();
    input.extend_from_slice(b"\x1b]52;c;d29ybGQ=\x1b\\");
    input.extend_from_slice(b"x");
    let out = parser.feed(&input);
    assert_eq!(
        out.completed_clipboard_forward,
        Some((2, b"\x1b]52;c;d29ybGQ=\x1b\\".to_vec()))
    );
    assert_eq!(out.residue, b"x");
}

#[test]
fn clipboard_and_barrier_forwards_are_independent() {
    let mut parser = StdinAnsiParser::new();
    parser.open_clipboard_forward(1);
    parser.open_forward(2);
    let out = parser.feed(b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\\x1b[?62;52c");
    assert_eq!(
        out.completed_forward,
        Some((2, b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\".to_vec()))
    );
    assert!(out.completed_clipboard_forward.is_none());
    assert_eq!(parser.active_clipboard_forward_token(), Some(1));
}

#[test]
fn clipboard_reply_is_not_accumulated_into_a_barrier_slot() {
    let mut parser = StdinAnsiParser::new();
    parser.open_clipboard_forward(1);
    parser.open_forward(2);
    let out = parser.feed(b"\x1b]52;c;aGk=\x1b\\");
    assert_eq!(
        out.completed_clipboard_forward,
        Some((1, b"\x1b]52;c;aGk=\x1b\\".to_vec()))
    );
    let out = parser.feed(b"\x1b[?62;52c");
    assert_eq!(
        out.completed_forward,
        Some((2, Vec::new())),
        "the clipboard payload must not leak into the barrier slot"
    );
}

#[test]
fn taking_the_clipboard_slot_hands_the_token_over() {
    let mut parser = StdinAnsiParser::new();
    parser.open_clipboard_forward(6);
    assert_eq!(
        parser.take_active_clipboard_forward(),
        Some((6, Vec::new()))
    );
    assert!(parser.active_clipboard_forward_token().is_none());
    assert_eq!(parser.take_active_clipboard_forward(), None);
}

#[test]
fn clipboard_timeout_only_fires_for_the_open_token() {
    let mut parser = StdinAnsiParser::new();
    parser.open_clipboard_forward(3);
    assert_eq!(parser.close_clipboard_forward_on_timeout(4), None);
    assert_eq!(
        parser.close_clipboard_forward_on_timeout(3),
        Some((3, Vec::new()))
    );
    assert_eq!(parser.close_clipboard_forward_on_timeout(3), None);
}

// =====================================================================
// Property-based tests (hegel)
// =====================================================================

use hegel::generators;

/// Real sequence fragments the parser's state machine branches on. Random
/// bytes alone almost never reach the OSC/CSI/APC/DCS/paste paths, so the
/// generator interleaves these with noise.
const SEQ_FRAGMENTS: &[&[u8]] = &[
    b"\x1b",
    b"\x1b]",
    b"\x1b[",
    b"\x1b_G",
    b"\x1b_",
    b"\x1bP26661n",
    b"\x1b[200~",
    b"\x1b[201~",
    b"\x1b\\",
    b"\x07",
    b"11;rgb:0000/0000/0000",
    b"10;rgb:ffff/ffff/ffff",
    b"4;12;rgb:1212/3434/5656",
    b"99;body",
    b"\x1b[4;720;1280t",
    b"\x1b[6;18;9t",
    b"\x1b[?2026;2$y",
    b"\x1b[?997;1n",
    b"\x1b[?62;4c",
    b"i=31;OK\x1b\\",
    b"abc",
];

/// Raw stdin as it arrives from the host terminal: real ANSI fragments
/// interleaved with arbitrary binary noise.
///
/// Piece count and noise size are bounded only for runtime sanity — the
/// resulting buffers stay in the low kilobytes, many orders of magnitude
/// below `PARTIAL_BUFFER_CAP_BYTES` (100MB), which cannot be reached in a
/// property test anyway.
#[hegel::composite]
fn stdin_bytes(tc: &hegel::TestCase) -> Vec<u8> {
    let pieces = tc.draw(generators::integers::<usize>().max_value(24));
    let mut out = Vec::new();
    for _ in 0..pieces {
        if tc.draw(generators::booleans()) {
            if tc.draw(generators::integers::<u8>().max_value(7)) == 0 {
                // A complete, well-formed nested-session frame — the shape
                // `nested_frame_split_at_every_boundary_is_extracted` covers
                // by hand.
                out.extend(announce_frame_and_payload().0);
            } else {
                let fragment = tc.draw(generators::sampled_from(SEQ_FRAGMENTS.to_vec()));
                out.extend_from_slice(fragment);
            }
        }
        out.extend(tc.draw(generators::binary().max_size(16)));
    }
    out
}

/// The whole observable output of driving a parser to exhaustion over a
/// sequence of chunks: everything it classified, plus everything it was
/// still holding when the stream went idle.
#[derive(Debug, PartialEq, Eq)]
struct Accumulated {
    replies: Vec<String>,
    nested_frames: Vec<Vec<u8>>,
    desktop_notifications: Vec<Vec<u8>>,
    residue: Vec<u8>,
    finalized: Vec<u8>,
}

fn drive(chunks: &[&[u8]]) -> Accumulated {
    let mut parser = StdinAnsiParser::new();
    let mut acc = Accumulated {
        replies: Vec::new(),
        nested_frames: Vec::new(),
        desktop_notifications: Vec::new(),
        residue: Vec::new(),
        finalized: Vec::new(),
    };
    for chunk in chunks {
        let out = parser.feed(chunk);
        // HostReply has no PartialEq; its Debug rendering is total and
        // faithful enough to compare two runs.
        acc.replies
            .extend(out.replies.iter().map(|r| format!("{:?}", r)));
        acc.nested_frames.extend(out.nested_frames);
        acc.desktop_notifications.extend(out.desktop_notifications);
        acc.residue.extend(out.residue);
    }
    acc.finalized = parser.finalize_force();
    acc
}

#[hegel::test]
fn feed_never_panics_on_arbitrary_stdin(tc: hegel::TestCase) {
    // Unbounded raw bytes: stdin is untrusted input from the host terminal.
    let bytes = tc.draw(generators::binary());
    let mut parser = StdinAnsiParser::new();
    let out = parser.feed(&bytes);
    let _ = parser.pending_partial();
    let _ = parser.finalize_fast_partial();
    let finalized = parser.finalize_force();
    // A single feed can only ever emit bytes it was given: residue plus
    // whatever is still buffered cannot exceed the input.
    assert!(
        out.residue.len() + finalized.len() <= bytes.len(),
        "feed of {:?} emitted {} residue + {} buffered bytes from {} input bytes",
        bytes,
        out.residue.len(),
        finalized.len(),
        bytes.len()
    );
}

#[hegel::test]
fn feed_never_panics_on_ansi_shaped_stdin(tc: hegel::TestCase) {
    let bytes = tc.draw(stdin_bytes());
    let mut parser = StdinAnsiParser::new();
    let _ = parser.feed(&bytes);
    let _ = parser.pending_partial();
    let _ = parser.finalize_force();
}

#[hegel::test(test_cases = 500)]
fn chunk_splitting_does_not_change_what_is_parsed(tc: hegel::TestCase) {
    let bytes = tc.draw(stdin_bytes());
    // Split points are bounded by the buffer length so they land on real
    // boundaries instead of being folded modulo into a few low offsets.
    let mut splits = tc
        .draw(generators::vecs(generators::integers::<usize>().max_value(bytes.len())).max_size(8));
    splits.sort_unstable();
    splits.dedup();

    let mut chunks: Vec<&[u8]> = Vec::new();
    let mut prev = 0;
    for split in &splits {
        chunks.push(&bytes[prev..*split]);
        prev = *split;
    }
    chunks.push(&bytes[prev..]);

    let whole = drive(&[&bytes[..]]);
    let split = drive(&chunks);
    assert_eq!(
        whole, split,
        "feeding {:?} whole differs from feeding it split at {:?}",
        bytes, splits
    );
}

#[test]
fn attachment_generation_expired_timeout_preserves_reused_forward_token() {
    let runtime = paused_runtime();
    let parser = Arc::new(Mutex::new(StdinAnsiParser::new()));
    {
        let mut parser = parser.lock().unwrap();
        parser.forward_generation = Some(1);
        parser.open_forward(7);
    }
    let fired = Arc::new(Mutex::new(false));
    let capture = fired.clone();
    schedule_timeout(
        runtime.handle(),
        parser.clone(),
        ForwardKind::Barrier,
        7,
        Duration::from_millis(500),
        move |_, _| *capture.lock().unwrap() = true,
    );
    {
        let mut parser = parser.lock().unwrap();
        parser.take_active(ForwardKind::Barrier);
        parser.forward_generation = Some(2);
        parser.open_forward(7);
    }
    runtime.block_on(async {
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(600)).await;
        tokio::task::yield_now().await;
    });
    assert!(!*fired.lock().unwrap());
    assert_eq!(parser.lock().unwrap().active_forward_token(), Some(7));
}

#[test]
fn attachment_generation_late_physical_replies_are_drained_before_recovery_queries() {
    for kind in [ForwardKind::Barrier, ForwardKind::Clipboard] {
        for timed_out in [false, true] {
            let mut parser = StdinAnsiParser::new();
            parser.begin_forward_generation(Some(1));
            parser.open(kind, 17);
            parser.queue_terminal_query(kind, b"A".to_vec());
            assert_eq!(parser.ready_terminal_queries().count(), 1);
            if timed_out {
                assert!(parser.close_on_timeout(kind, 17).is_some());
            }
            parser.begin_forward_generation(Some(2));
            parser.open(kind, 18);
            parser.queue_terminal_query(kind, b"B".to_vec());
            assert_eq!(parser.ready_terminal_queries().count(), 0);
            let (old, new) = match kind {
                ForwardKind::Barrier => (
                    b"\x1b[4;100;100t\x1b[?1c".as_slice(),
                    b"\x1b[4;200;200t\x1b[?1c".as_slice(),
                ),
                ForwardKind::Clipboard => (
                    b"\x1b]52;c;b2xk\x1b\\".as_slice(),
                    b"\x1b]52;c;bmV3\x1b\\".as_slice(),
                ),
            };
            let discarded = parser.feed(old);
            assert!(
                discarded.completed_forward.is_none()
                    && discarded.completed_clipboard_forward.is_none()
            );
            let query = parser.ready_terminal_queries().collect::<Vec<_>>();
            assert_eq!(
                query,
                vec![[b"B".as_slice(), kind.barrier_suffix()].concat()]
            );
            let reply = parser.feed(new);
            let (token, bytes) = reply
                .completed_forward
                .or(reply.completed_clipboard_forward)
                .unwrap();
            assert_eq!(token, 18);
            assert_eq!(bytes, new.strip_suffix(b"\x1b[?1c").unwrap_or(new));
        }
    }
}

#[test]
fn attachment_generation_timeout_cancels_the_unsent_recovery_query() {
    for kind in [ForwardKind::Barrier, ForwardKind::Clipboard] {
        let mut parser = StdinAnsiParser::new();
        parser.begin_forward_generation(Some(1));
        parser.open(kind, 1);
        parser.queue_terminal_query(kind, b"A".to_vec());
        assert_eq!(parser.ready_terminal_queries().count(), 1);
        parser.begin_forward_generation(Some(2));
        parser.open(kind, 1);
        parser.queue_terminal_query(kind, b"B".to_vec());
        assert_eq!(parser.ready_terminal_queries().count(), 0);
        assert_eq!(parser.close_on_timeout(kind, 1), Some((1, vec![])));

        let late = match kind {
            ForwardKind::Barrier => b"\x1b[?1c".as_slice(),
            ForwardKind::Clipboard => b"\x1b]52;c;b2xk\x1b\\".as_slice(),
        };
        let output = parser.feed(late);
        assert!(output.completed_forward.is_none() && output.completed_clipboard_forward.is_none());
        assert_eq!(parser.ready_terminal_queries().count(), 0);
        parser.open(kind, 2);
        parser.queue_terminal_query(kind, b"C".to_vec());
        assert_eq!(
            parser.ready_terminal_queries().collect::<Vec<_>>(),
            vec![[b"C".as_slice(), kind.barrier_suffix()].concat()]
        );
    }
}

#[test]
fn recovered_terminal_drains_fragmented_query_replies_at_every_split() {
    for (kind, old) in [
        (ForwardKind::Clipboard, b"\x1b]52;c;b2xk\x07".as_slice()),
        (ForwardKind::Clipboard, b"\x1b]52;c;b2xk\x1b\\"),
        (ForwardKind::Barrier, b"\x1b[?62;1;6c"),
        (ForwardKind::Barrier, b"\x1b[>0;276;0c"),
        (ForwardKind::Barrier, b"\x1b[c"),
    ] {
        for split in 1..old.len() {
            let mut parser = StdinAnsiParser::new();
            parser.begin_forward_generation(Some(1));
            parser.open(kind, 6);
            parser.queue_terminal_query(kind, b"A".to_vec());
            assert_eq!(parser.ready_terminal_queries().count(), 1);
            parser.feed(&old[..split]);
            parser.reset_for_recovery();
            parser.begin_forward_generation(Some(2));
            parser.open(kind, 7);
            parser.queue_terminal_query(kind, b"B".to_vec());
            assert_eq!(parser.ready_terminal_queries().count(), 0);
            for byte in &old[split..] {
                let result = parser.feed(std::slice::from_ref(byte));
                assert!(
                    result.completed_clipboard_forward.is_none()
                        && result.completed_forward.is_none()
                );
                assert!(result.residue.is_empty(), "{old:?} split at {split}");
                parser.reset_for_recovery();
            }
            assert_eq!(
                parser.ready_terminal_queries().count(),
                1,
                "{old:?} split at {split}"
            );
            let (reply, expected) = match kind {
                ForwardKind::Clipboard => (
                    b"\x1b]52;c;bmV3\x1b\\".as_slice(),
                    b"\x1b]52;c;bmV3\x1b\\".as_slice(),
                ),
                ForwardKind::Barrier => (
                    b"\x1b[6;20;10t\x1b[?1c".as_slice(),
                    b"\x1b[6;20;10t".as_slice(),
                ),
            };
            let output = parser.feed(reply);
            assert_eq!(
                output
                    .completed_clipboard_forward
                    .or(output.completed_forward),
                Some((7, expected.to_vec()))
            );
        }
    }
}

#[test]
fn recovered_terminal_keeps_live_observations_while_a_barrier_drains() {
    let mut parser = StdinAnsiParser::new();
    parser.begin_forward_generation(Some(1));
    parser.open_forward(1);
    parser.queue_terminal_query(ForwardKind::Barrier, b"A".to_vec());
    assert_eq!(parser.ready_terminal_queries().count(), 1);
    parser.begin_forward_generation(Some(2));
    parser.open_forward(2);
    parser.queue_terminal_query(ForwardKind::Barrier, b"B".to_vec());
    let output = parser.feed(b"\x1b[?997;2n\x1b[6;20;10t\x1b]11;rgb:1/2/3\x07");
    assert!(matches!(&output.replies[..], [
        HostReply::HostTerminalThemeChanged(yinz_types::data::HostTerminalThemeMode::Light),
        HostReply::PixelDimensions(pixels), HostReply::BackgroundColor(color)
    ] if pixels.character_cell_size.as_ref().is_some_and(|size| size.height == 20 && size.width == 10) && color == "rgb:1/2/3"));
    assert!(output.completed_forward.is_none() && output.residue.is_empty());
    parser.feed(b"\x1b[?1c");
    assert_eq!(parser.ready_terminal_queries().count(), 1);
    assert_eq!(
        parser.feed(b"\x1b[?1c").completed_forward,
        Some((2, vec![]))
    );
}

#[test]
fn recovered_terminal_keeps_the_new_startup_probe_separate_from_a_pane_query() {
    let mut source = StdinAnsiParser::new();
    source.open_forward(1);
    source.queue_terminal_query(ForwardKind::Barrier, b"A".to_vec());
    assert_eq!(source.ready_terminal_queries().count(), 1);
    let mut destination = source.take_for_next_attachment();
    destination.startup_barrier_sent(None);
    destination.open_forward(2);
    destination.queue_terminal_query(ForwardKind::Barrier, b"B".to_vec());
    let old = destination.feed(b"\x1b[4;10;10t\x1b[?1c");
    assert!(old.completed_forward.is_none());
    assert!(!destination.has_ready_terminal_query());
    let startup = destination.feed(b"\x1b[4;20;20t\x1b[?1;4c");
    assert!(startup.completed_forward.is_none());
    assert!(matches!(
        startup.replies.last(),
        Some(HostReply::SixelSupport(true))
    ));
    assert_eq!(destination.ready_terminal_queries().count(), 1);
    let own = b"\x1b[4;30;30t";
    let output = destination.feed(&[own.as_slice(), b"\x1b[?1c"].concat());
    assert_eq!(output.completed_forward, Some((2, own.to_vec())));
}

fn source_clipboard_partial(prefix: &[u8]) -> StdinAnsiParser {
    let mut parser = StdinAnsiParser::new();
    parser.begin_forward_generation(Some(1));
    parser.open_clipboard_forward(7);
    parser.queue_terminal_query(ForwardKind::Clipboard, b"A".to_vec());
    assert_eq!(parser.ready_terminal_queries().count(), 1);
    assert!(parser.feed(prefix).residue.is_empty());
    parser
}

#[test]
fn clipboard_idle_guard_preserves_physical_reply_across_handoff() {
    for old in [b"\x1b]52;c;b2xk\x07".as_slice(), b"\x1b]52;c;b2xk\x1b\\"] {
        for split in 1..old.len() {
            for handoff in [false, true] {
                let mut parser = source_clipboard_partial(&old[..split]);
                let flushed = match parser.pending_partial() {
                    PendingPartial::None => panic!("missing partial at {split}"),
                    PendingPartial::ReplyInProgress => parser.finalize_force(),
                    _ => parser.finalize_fast_partial(),
                };
                if split < b"\x1b]52;".len() {
                    assert_eq!(flushed, &old[..split]);
                } else {
                    assert!(flushed.is_empty());
                }
                if handoff {
                    parser = parser.take_for_next_attachment();
                    parser.begin_forward_generation(Some(2));
                    parser.open_clipboard_forward(7);
                    parser.queue_terminal_query(ForwardKind::Clipboard, b"B".to_vec());
                    assert!(!parser.has_ready_terminal_query());
                }
                let output = parser.feed(&old[split..]);
                assert!(output.residue.is_empty());
                if handoff {
                    assert!(output.completed_clipboard_forward.is_none());
                    assert_eq!(parser.ready_terminal_queries().collect::<Vec<_>>(), [b"B"]);
                    let own = b"\x1b]52;c;bmV3\x1b\\";
                    assert_eq!(
                        parser.feed(own).completed_clipboard_forward,
                        Some((7, own.to_vec()))
                    );
                } else {
                    assert_eq!(
                        output.completed_clipboard_forward,
                        Some((7, b"\x1b]52;c;b2xk\x1b\\".to_vec()))
                    );
                }
            }
        }
    }
}

#[test]
fn unfinished_clipboard_releases_interrupts_and_keeps_query_fenced() {
    for keys in [
        b"\r".as_slice(),
        b"\x03",
        b"\x1b[A",
        b" hello",
        b"\x03hello",
    ] {
        for split in 0..=keys.len() {
            let mut parser = source_clipboard_partial(b"\x1b]52;c;b2xk");
            parser.input_stamp_changed(Some((1, true)), Some((2, true)));
            parser.begin_forward_generation(Some(2));
            let mut typed = parser.feed(&keys[..split]).residue;
            typed.extend(parser.feed(&keys[split..]).residue);
            match parser.pending_partial() {
                PendingPartial::None => {},
                PendingPartial::ReplyInProgress => typed.extend(parser.finalize_force()),
                _ => typed.extend(parser.finalize_fast_partial()),
            }
            assert_eq!(typed, keys, "split {split}");
            assert_eq!(parser.pending_partial(), PendingPartial::None);
            parser.open_clipboard_forward(1);
            parser.queue_terminal_query(ForwardKind::Clipboard, b"B".to_vec());
            assert!(!parser.has_ready_terminal_query());
        }
    }
}

#[test]
fn unfinished_clipboard_keeps_an_ambiguous_suffix_out_of_keyboard_input() {
    let mut parser = source_clipboard_partial(b"\x1b]52;c;");
    parser = parser.take_for_next_attachment();
    parser.open_clipboard_forward(8);
    parser.queue_terminal_query(ForwardKind::Clipboard, b"B".to_vec());
    // These bytes could equally be typing or the body of the retired answer.
    assert!(parser.feed(b"test").residue.is_empty());
    assert!(parser.finalize_force().is_empty());
    assert!(!parser.has_ready_terminal_query());
    let late = parser.feed(b"\x07");
    assert!(late.completed_clipboard_forward.is_none() && late.residue.is_empty());
    assert_eq!(parser.ready_terminal_queries().collect::<Vec<_>>(), [b"B"]);
}

#[test]
fn first_repaint_keeps_fragmented_host_observations_during_barrier_drain() {
    for report in [
        b"\x1b]11;rgb:1/2/3\x07".as_slice(),
        b"\x1b]10;rgb:1/2/3\x1b\\",
        b"\x1b]4;3;rgb:1/2/3\x07",
        b"\x1b[?997;2n",
        b"\x1b[6;20;10t",
    ] {
        let expected = format!("{:?}", StdinAnsiParser::new().feed(report).replies);
        for split in 1..report.len() {
            let mut parser = StdinAnsiParser::new();
            parser.begin_forward_generation(Some(1));
            parser.open_forward(1);
            parser.queue_terminal_query(ForwardKind::Barrier, b"A".to_vec());
            assert_eq!(parser.ready_terminal_queries().count(), 1);
            parser.begin_forward_generation(Some(2));
            parser.open_forward(2);
            parser.queue_terminal_query(ForwardKind::Barrier, b"B".to_vec());
            assert!(parser.feed(&report[..split]).residue.is_empty());
            parser.input_stamp_changed(Some((2, false)), Some((2, true)));
            let output = parser.feed(&report[split..]);
            assert_eq!(output.replies.len(), 1, "{report:?} at {split}");
            assert_eq!(format!("{:?}", output.replies), expected);
            assert!(output.residue.is_empty() && output.completed_forward.is_none());
            assert!(!parser.has_ready_terminal_query());
            assert!(parser.feed(b"\x1b[?1c").completed_forward.is_none());
            assert_eq!(parser.ready_terminal_queries().count(), 1);
            assert_eq!(
                parser.feed(b"\x1b[?1c").completed_forward,
                Some((2, vec![]))
            );
        }
    }
}

#[test]
fn idle_guard_within_retired_header_keeps_bytes_off_the_keyboard() {
    for (kind, reply) in [
        (ForwardKind::Clipboard, b"\x1b]52;c;b2xk\x07".as_slice()),
        (ForwardKind::Clipboard, b"\x1b]52;c;b2xk\x1b\\"),
        (ForwardKind::Barrier, b"\x1b[?62;1;6c"),
    ] {
        for split in 1..reply.len() {
            let mut source = StdinAnsiParser::new();
            source.open(kind, 1);
            source.queue_terminal_query(kind, b"A".to_vec());
            assert_eq!(source.ready_terminal_queries().count(), 1);
            assert!(source.feed(&reply[..split]).residue.is_empty());
            let mut destination = source.take_for_next_attachment();
            destination.open(kind, 1);
            destination.queue_terminal_query(kind, b"B".to_vec());
            for byte in &reply[split..] {
                assert!(!destination.has_ready_terminal_query());
                let output = destination.feed(std::slice::from_ref(byte));
                assert!(output.residue.is_empty());
                assert!(output.completed_forward.is_none());
                assert!(output.completed_clipboard_forward.is_none());
                assert_eq!(destination.pending_partial(), PendingPartial::None);
                assert!(destination.finalize_fast_partial().is_empty());
                assert!(destination.finalize_force().is_empty());
            }
            assert_eq!(destination.ready_terminal_queries().count(), 1);
            let own = destination.feed(reply);
            assert_eq!(
                own.completed_forward
                    .or(own.completed_clipboard_forward)
                    .unwrap()
                    .0,
                1
            );
            assert_eq!(destination.feed(b"k").residue, b"k");
        }
    }
}

#[test]
fn idle_guard_during_first_repaint_preserves_slow_host_reports() {
    for report in [
        b"\x1b]11;rgb:1/2/3\x07".as_slice(),
        b"\x1b]10;rgb:1/2/3\x1b\\",
        b"\x1b]4;3;rgb:1/2/3\x07",
        b"\x1b]4;23;#abcdef012345\x1b\\",
        b"\x1b]10;rgb:1a/bcd/3ef0\x07",
        b"\x1b]4;23;red\x07",
        b"\x1b[?997;2n",
        b"\x1b[6;20;10t",
    ] {
        let expected = StdinAnsiParser::new().feed(report).replies;
        for split in 1..report.len() {
            let mut parser = StdinAnsiParser::new();
            parser.begin_forward_generation(Some(1));
            parser.open_forward(1);
            parser.queue_terminal_query(ForwardKind::Barrier, b"A".to_vec());
            assert_eq!(parser.ready_terminal_queries().count(), 1);
            parser.begin_forward_generation(Some(2));
            parser.open_forward(2);
            parser.queue_terminal_query(ForwardKind::Barrier, b"B".to_vec());
            assert!(parser.feed(&report[..split]).residue.is_empty());
            let flushed = parser.finalize_force();
            if split > 2 {
                assert!(flushed.is_empty(), "{report:?} at {split}");
            }
            parser.input_stamp_changed(Some((2, false)), Some((2, true)));
            let mut observed = Vec::new();
            for byte in &report[split..] {
                let output = parser.feed(std::slice::from_ref(byte));
                observed.extend(output.replies);
                assert!(output.residue.is_empty() && output.completed_forward.is_none());
                assert!(parser.finalize_force().is_empty(), "{report:?} at {split}");
            }
            assert_eq!(observed.len(), 1);
            assert_eq!(format!("{observed:?}"), format!("{expected:?}"));
            assert_eq!(parser.feed(b"k").residue, b"k");
            assert!(!parser.has_ready_terminal_query());
            assert!(parser.feed(b"\x1b[?1c").completed_forward.is_none());
            assert_eq!(parser.ready_terminal_queries().count(), 1);
            assert_eq!(
                parser.feed(b"\x1b[?1c").completed_forward,
                Some((2, vec![]))
            );
        }
    }
}

#[test]
fn first_repaint_delivers_typing_while_a_host_color_awaits_its_terminator() {
    for (report, keys, terminator) in [
        (
            b"\x1b]11;rgb:1/2/3".as_slice(),
            b"zoom".as_slice(),
            b"\x07".as_slice(),
        ),
        (b"\x1b]10;rgb:ffff/a/12", b"fade away", b"\x1b\\"),
        (b"\x1b]4;19;#abc", b"face zoom\r", b"\x07"),
    ] {
        let complete = [report, terminator].concat();
        let expected = StdinAnsiParser::new().feed(&complete).replies;
        for split in 0..=keys.len() {
            let mut parser = StdinAnsiParser::new();
            parser.begin_forward_generation(Some(1));
            parser.open_forward(1);
            parser.queue_terminal_query(ForwardKind::Barrier, b"A".to_vec());
            assert_eq!(parser.ready_terminal_queries().count(), 1);
            parser.begin_forward_generation(Some(2));
            parser.open_forward(2);
            parser.queue_terminal_query(ForwardKind::Barrier, b"B".to_vec());
            assert!(parser.feed(report).residue.is_empty());
            parser.input_stamp_changed(Some((2, false)), Some((2, true)));

            let mut typed = parser.feed(&keys[..split]).residue;
            typed.extend(parser.finalize_force());
            typed.extend(parser.feed(&keys[split..]).residue);
            typed.extend(parser.finalize_force());
            assert_eq!(typed, keys, "split {split}");
            let mut observed = Vec::new();
            for byte in terminator {
                let out = parser.feed(std::slice::from_ref(byte));
                assert!(out.residue.is_empty() && out.completed_forward.is_none());
                observed.extend(out.replies);
            }
            assert_eq!(format!("{observed:?}"), format!("{expected:?}"));
            assert!(!parser.has_ready_terminal_query());
            assert!(parser.feed(b"\x1b[?1c").completed_forward.is_none());
            assert_eq!(parser.ready_terminal_queries().count(), 1);
        }
    }
}

#[test]
fn first_repaint_keeps_keys_and_color_in_one_read_in_order() {
    let mut parser = StdinAnsiParser::new();
    assert!(parser.feed(b"\x1b]11;rgb:1/2/3").residue.is_empty());
    parser.input_stamp_changed(Some((2, false)), Some((2, true)));
    let out = parser.feed(b"fade zoom\x07!\x1b]10;rgb:a/b/c\x1b\\");
    assert_eq!(out.residue, b"fade zoom!");
    assert!(matches!(&out.replies[..], [
        HostReply::BackgroundColor(bg), HostReply::ForegroundColor(fg)
    ] if bg == "rgb:1/2/3" && fg == "rgb:a/b/c"));
}

#[test]
fn repaint_color_terminator_releases_only_incomplete_hex_digits() {
    for (before, after, color, keys) in [
        ("#abc", "d", "#abc", "d"),
        ("#abcd", "e", "#abc", "e"),
        ("#abcde", "", "#abc", ""),
        ("#abcd", "ef", "#abcdef", ""),
        ("#abcdef", "01", "#abcdef", "01"),
        ("#abc", "def0", "#abcdef", "0"),
        ("#ab", "cdef", "#abcdef", ""),
    ] {
        for end in [b"\x07".as_slice(), b"\x1b\\"] {
            let mut parser = StdinAnsiParser::new();
            parser.feed(format!("\x1b]11;{before}").as_bytes());
            parser.input_stamp_changed(Some((2, false)), Some((2, true)));
            for byte in after.bytes() {
                assert!(parser.feed(&[byte]).residue.is_empty());
                assert!(parser.finalize_force().is_empty());
            }
            for byte in &end[..end.len() - 1] {
                assert!(parser.feed(&[*byte]).residue.is_empty());
                assert!(parser.finalize_force().is_empty());
            }
            let output = parser.feed(&[end[end.len() - 1], b'!']);
            assert_eq!(output.residue, format!("{keys}!").as_bytes());
            assert!(
                matches!(&output.replies[..], [HostReply::BackgroundColor(value)] if value == color)
            );
        }
    }
}

#[test]
fn repaint_color_interruption_replays_only_fresh_surplus_digits() {
    for interrupt in [b"z".as_slice(), b"\x1b[A"] {
        let mut parser = StdinAnsiParser::new();
        parser.feed(b"\x1b]11;#abcd");
        parser.input_stamp_changed(Some((2, false)), Some((2, true)));
        assert!(parser.feed(b"e").residue.is_empty());
        let output = parser.feed(interrupt);
        assert_eq!(output.residue, [b"e", interrupt].concat());
        if interrupt == b"z" {
            assert!(matches!(
                &parser.feed(b"\x07").replies[..],
                [HostReply::BackgroundColor(value)] if value == "#abc"
            ));
        }
    }
}

#[test]
fn interrupted_hex_color_keeps_its_pre_repaint_digits_until_termination() {
    for (before, interrupt, continuation, expected_color, keys) in [
        ("#abcd", "z", "ef", "#abcdef", "z!"),
        ("#abcde", "z", "f", "#abcdef", "z!"),
        ("#abcd", "ez", "ef", "#abcdef", "ez!"),
        ("#abcd", "z", "", "#abc", "z!"),
        ("#abcdef0", "z", "12", "#abcdef012", "z!"),
    ] {
        for end in [b"\x07".as_slice(), b"\x1b\\"] {
            let tail = [interrupt.as_bytes(), continuation.as_bytes(), end, b"!"].concat();
            for split in 0..=tail.len() {
                let mut parser = StdinAnsiParser::new();
                parser.feed(format!("\x1b]11;{before}").as_bytes());
                parser.input_stamp_changed(Some((2, false)), Some((2, true)));
                let mut output = parser.feed(&tail[..split]);
                let rest = parser.feed(&tail[split..]);
                output.residue.extend(rest.residue);
                output.replies.extend(rest.replies);
                assert_eq!(output.residue, keys.as_bytes(), "{before}, split {split}");
                assert!(matches!(&output.replies[..],
                    [HostReply::BackgroundColor(color)] if color == expected_color
                ));
            }
        }
    }
}

#[test]
fn repaint_color_interleaved_key_preserves_the_unfinished_body() {
    for (before, suffix, color, keys) in [
        ("rgb:1111/", "z2222/3333", "rgb:1111/2222/3333", "z!"),
        ("rgb:1/2/", "34f0z", "rgb:1/2/34f0", "z!"),
        ("rgb:1/2/", "3zoomfade", "rgb:1/2/3", "zoomfade!"),
    ] {
        for end in [b"\x07".as_slice(), b"\x1b\\"] {
            let tail = [suffix.as_bytes(), end, b"!"].concat();
            for split in 0..=tail.len() {
                let mut parser = StdinAnsiParser::new();
                parser.feed(format!("\x1b]11;{before}").as_bytes());
                parser.input_stamp_changed(Some((2, false)), Some((2, true)));
                let mut output = parser.feed(&tail[..split]);
                let rest = parser.feed(&tail[split..]);
                output.residue.extend(rest.residue);
                output.replies.extend(rest.replies);
                assert_eq!(output.residue, keys.as_bytes(), "split {split}");
                assert!(
                    matches!(&output.replies[..], [HostReply::BackgroundColor(value)] if value == color)
                );
            }
        }
    }
}

#[test]
fn repaint_color_resumed_body_keeps_a_slow_string_terminator_off_the_keyboard() {
    let mut parser = StdinAnsiParser::new();
    parser.feed(b"\x1b]11;rgb:1111/");
    parser.input_stamp_changed(Some((2, false)), Some((2, true)));
    assert_eq!(parser.feed(b"z").residue, b"z");
    assert!(parser.feed(b"2222/3333\x1b").residue.is_empty());
    assert!(parser.finalize_fast_partial().is_empty());
    assert!(parser.finalize_force().is_empty());
    let output = parser.feed(b"\\!");
    assert_eq!(output.residue, b"!");
    assert!(
        matches!(&output.replies[..], [HostReply::BackgroundColor(value)] if value == "rgb:1111/2222/3333")
    );
}

#[test]
fn repaint_color_interrupted_escape_uses_the_keyboard_deadline() {
    for tail in [b"\\!".as_slice(), b"x\x07!"] {
        let mut parser = StdinAnsiParser::new();
        parser.feed(b"\x1b]11;rgb:1/2/3");
        parser.input_stamp_changed(Some((2, false)), Some((2, true)));
        assert_eq!(parser.feed(b"\r").residue, b"\r");
        assert!(parser.feed(b"\x1b").residue.is_empty());
        assert_eq!(parser.pending_partial(), PendingPartial::LoneEsc);
        assert_eq!(parser.finalize_fast_partial(), b"\x1b");
        assert_eq!(parser.pending_partial(), PendingPartial::None);
        assert!(parser.finalize_fast_partial().is_empty());
        let output = parser.feed(tail);
        let keys = if tail.starts_with(b"x") {
            &b"x!"[..]
        } else {
            b"!"
        };
        assert_eq!(output.residue, keys);
        assert!(
            matches!(&output.replies[..], [HostReply::BackgroundColor(value)] if value == "rgb:1/2/3")
        );
    }
}
