use super::{
    build_startup_query_string, drain_partial_to_keyboard, extract_focus_reports,
    forget_held_modifiers, next_wakeup, parse_keyboard_bytes, schedule_finalization,
    LONE_ESC_FLUSH_INTERVAL, PIXEL_SIZE_QUERY,
};
use crate::{stdin_ansi_parser::StdinAnsiParser, InputInstruction};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use yinz_key_codec::vendored::termwiz::input::{InputEvent, InputParser, KeyCode, Modifiers};
use yinz_types::keys::BareKey;

fn parse_stdin_chunks(chunks: &[&[u8]]) -> Vec<InputInstruction> {
    parse_stdin_chunks_reporting_modifiers(chunks, false)
}

#[test]
fn extended_kitty_function_keys_survive_every_input_split() {
    for number in 13..=35 {
        let sequence = format!("\x1b[{};16:2u", 57363 + number);
        for split in 0..=sequence.len() {
            let instructions = parse_stdin_chunks_reporting_modifiers(
                &[&sequence.as_bytes()[..split], &sequence.as_bytes()[split..]],
                true,
            );
            assert_eq!(instructions.len(), 1, "{instructions:?}");
            match &instructions[0] {
                InputInstruction::KeyWithModifierEvent(key, bytes, true) => {
                    assert_eq!(
                        *key,
                        yinz_types::keys::KeyWithModifier::new(BareKey::F(number as u8))
                            .with_shift_modifier()
                            .with_alt_modifier()
                            .with_ctrl_modifier()
                            .with_super_modifier(),
                    );
                    assert_eq!(*bytes, format!("\x1b[{};16u", 57363 + number).into_bytes());
                },
                other => panic!("expected a function key, got {other:?}"),
            }
        }
    }
}

#[test]
fn unsupported_kitty_functional_codes_do_not_leak_into_following_text() {
    for code in [57344, 57416, 57427, 57455, 63743]
        .into_iter()
        .chain(57428..=57440)
    {
        let sequence = format!("\x1b[{code};5uX");
        for split in 0..=sequence.len() {
            let instructions =
                parse_stdin_chunks(&[&sequence.as_bytes()[..split], &sequence.as_bytes()[split..]]);
            assert_eq!(instructions.len(), 1, "{code}: {instructions:?}");
            match &instructions[0] {
                InputInstruction::KeyEvent(InputEvent::Key(key), bytes) => {
                    assert_eq!(key.key, KeyCode::Char('X'));
                    assert_eq!(bytes, b"X");
                },
                other => panic!("expected following text, got {other:?}"),
            }
        }
    }
}

fn parse_stdin_chunks_reporting_modifiers(
    chunks: &[&[u8]],
    report_modifier_keys: bool,
) -> Vec<InputInstruction> {
    use std::collections::BTreeSet;
    let mut ansi_parser = StdinAnsiParser::new();
    let mut input_parser = InputParser::new();
    let mut termwiz_buffer = vec![];
    let mut held_modifiers = BTreeSet::new();
    let mut instructions = vec![];
    for chunk in chunks {
        let output = ansi_parser.feed(chunk);
        assert!(output.replies.is_empty());
        let (mut parsed, _) = parse_keyboard_bytes(
            &mut input_parser,
            &mut termwiz_buffer,
            &output.residue,
            true,
            true,
            report_modifier_keys,
            &mut held_modifiers,
        );
        instructions.append(&mut parsed);
    }
    let (mut flushed, _) = parse_keyboard_bytes(
        &mut input_parser,
        &mut termwiz_buffer,
        &[],
        true,
        false,
        report_modifier_keys,
        &mut held_modifiers,
    );
    instructions.append(&mut flushed);
    instructions
}

fn assert_mixed_kitty_events(instructions: &[InputInstruction]) {
    assert_eq!(instructions.len(), 3, "got {instructions:?}");
    match &instructions[0] {
        InputInstruction::KeyEvent(InputEvent::Key(key), raw_bytes) => {
            assert_eq!(key.key, KeyCode::Char('x'));
            assert_eq!(raw_bytes, b"x");
        },
        other => panic!("expected plain x, got {other:?}"),
    }
    match &instructions[1] {
        InputInstruction::KeyWithModifierEvent(key, raw_bytes, true) => {
            assert!(key.is_key_with_ctrl_modifier(BareKey::Char('a')));
            assert_eq!(raw_bytes, b"\x1b[97;5u");
        },
        other => panic!("expected Kitty Ctrl-a, got {other:?}"),
    }
    match &instructions[2] {
        InputInstruction::KeyEvent(InputEvent::Key(key), raw_bytes) => {
            assert_eq!(key.key, KeyCode::Char('y'));
            assert_eq!(raw_bytes, b"y");
        },
        other => panic!("expected plain y, got {other:?}"),
    }
}

#[test]
fn finalization_deadline_tracks_pending_input_and_preserves_the_reply_guard() {
    let parser = Arc::new(Mutex::new(StdinAnsiParser::new()));
    let mut deadline = None;
    let mut reply_started = None;

    schedule_finalization(&parser, false, &mut deadline, &mut reply_started);
    assert!(deadline.is_none());
    assert!(reply_started.is_none());

    schedule_finalization(&parser, true, &mut deadline, &mut reply_started);
    assert!(deadline.is_some());
    assert!(reply_started.is_none());

    // A reply-only chunk still restarts an existing keyboard flush window.
    let past = Instant::now() - Duration::from_secs(1);
    deadline = Some(past);
    schedule_finalization(&parser, false, &mut deadline, &mut reply_started);
    assert!(deadline.is_some_and(|at| at > past));

    deadline = None;
    parser.lock().unwrap().feed(b"\x1b]11;rgb:");
    schedule_finalization(&parser, false, &mut deadline, &mut reply_started);
    assert!(deadline.is_some());
    assert!(reply_started.is_some());

    reply_started = Some(past);
    schedule_finalization(&parser, false, &mut deadline, &mut reply_started);
    assert_eq!(reply_started, Some(past));

    parser.lock().unwrap().feed(b"ffff/ffff/ffff\x07");
    schedule_finalization(&parser, false, &mut deadline, &mut reply_started);
    assert!(deadline.is_some());
    assert!(reply_started.is_none());

    // Draining the keyboard window returns the loop to an idle state.
    deadline = None;
    schedule_finalization(&parser, false, &mut deadline, &mut reply_started);
    assert!(deadline.is_none());
}

#[test]
fn an_idle_loop_with_no_deadline_parks_forever() {
    assert_eq!(next_wakeup(Instant::now(), None, None), None);
}

#[test]
fn a_nearer_probe_deadline_does_not_shorten_the_flush_window_it_only_wakes_earlier() {
    let now = Instant::now();
    let wait = next_wakeup(
        now,
        Some(now + LONE_ESC_FLUSH_INTERVAL),
        Some(now + Duration::from_millis(5)),
    );
    // The wake-up is the probe's, and the caller checks `finalize_at` before draining, so
    // the escape window keeps its full 50ms.
    assert_eq!(wait, Some(Duration::from_millis(5)));
}

#[test]
fn a_distant_probe_deadline_leaves_the_flush_window_in_charge() {
    let now = Instant::now();
    let wait = next_wakeup(
        now,
        Some(now + LONE_ESC_FLUSH_INTERVAL),
        Some(now + Duration::from_secs(2)),
    );
    assert_eq!(wait, Some(LONE_ESC_FLUSH_INTERVAL));
}

#[test]
fn each_deadline_is_used_on_its_own_when_the_other_is_absent() {
    let now = Instant::now();
    assert_eq!(
        next_wakeup(now, Some(now + LONE_ESC_FLUSH_INTERVAL), None),
        Some(LONE_ESC_FLUSH_INTERVAL)
    );
    assert_eq!(
        next_wakeup(now, None, Some(now + Duration::from_secs(2))),
        Some(Duration::from_secs(2))
    );
}

#[test]
fn a_deadline_already_passed_wakes_immediately_rather_than_underflowing() {
    let now = Instant::now();
    let past = now - Duration::from_millis(10);
    assert_eq!(next_wakeup(now, Some(past), None), Some(Duration::ZERO));
}

#[test]
fn mixed_kitty_and_plain_keys_keep_order_and_raw_bytes_at_every_split() {
    let input = b"x\x1b[97;5uy";
    for split in 0..=input.len() {
        let instructions = parse_stdin_chunks(&[&input[..split], &input[split..]]);
        assert_mixed_kitty_events(&instructions);
    }
}

#[test]
fn consecutive_kitty_keys_in_one_read_are_dispatched_separately() {
    let instructions = parse_stdin_chunks(&[b"\x1b[97;5u\x1b[98;2u"]);
    assert_eq!(instructions.len(), 2, "got {instructions:?}");
    match &instructions[0] {
        InputInstruction::KeyWithModifierEvent(key, raw_bytes, true) => {
            assert!(key.is_key_with_ctrl_modifier(BareKey::Char('a')));
            assert_eq!(raw_bytes, b"\x1b[97;5u");
        },
        other => panic!("expected Kitty Ctrl-a, got {other:?}"),
    }
    match &instructions[1] {
        InputInstruction::KeyWithModifierEvent(key, raw_bytes, true) => {
            // Shift-b resolves to an unmodified B, the shape the legacy path produces for the
            // same keypress. The host's own bytes still ride along untouched here, because
            // `report_modifier_keys` is off for this harness.
            assert!(key.is_key_without_modifier(BareKey::Char('B')));
            assert_eq!(raw_bytes, b"\x1b[98;2u");
        },
        other => panic!("expected Kitty Shift-b, got {other:?}"),
    }
}

/// Under `report_modifier_keys` every printable key arrives as a CSI sequence, and the bytes
/// that ride along are re-serialised from the resolved key rather than passed through. They
/// have to come out as the text the key types: `screen`'s `WriteCharacter` hands them
/// verbatim to `update_active_tab_name`, `update_active_pane_name` and `update_search_term`,
/// so anything still escape-shaped is typed into a name as literal characters.
#[test]
fn under_report_modifier_keys_a_printable_key_carries_the_text_it_types() {
    for (input, expected_key, expected_bytes) in [
        (&b"\x1b[97u"[..], BareKey::Char('a'), &b"a"[..]),
        (&b"\x1b[116:84;2u"[..], BareKey::Char('T'), &b"T"[..]),
        (&b"\x1b[49:33;2u"[..], BareKey::Char('!'), &b"!"[..]),
        (&b"\x1b[32u"[..], BareKey::Char(' '), &b" "[..]),
    ] {
        let instructions = parse_stdin_chunks_reporting_modifiers(&[input], true);
        assert_eq!(instructions.len(), 1, "{input:?}: {instructions:?}");
        match &instructions[0] {
            InputInstruction::KeyWithModifierEvent(key, raw_bytes, true) => {
                assert!(
                    key.is_key_without_modifier(expected_key),
                    "{input:?} resolved to {key:?}"
                );
                assert_eq!(raw_bytes, expected_bytes, "{input:?}");
            },
            other => panic!("{input:?}: expected a kitty key, got {other:?}"),
        }
    }
}

/// A chord has no text to type, so it keeps an escape encoding - and the `is_typable_text`
/// guard on the server side is what keeps that out of a name.
#[test]
fn under_report_modifier_keys_a_chord_keeps_its_escape_encoding() {
    let instructions = parse_stdin_chunks_reporting_modifiers(&[b"\x1b[97;3u"], true);
    match &instructions[0] {
        InputInstruction::KeyWithModifierEvent(key, raw_bytes, true) => {
            assert!(key.is_key_with_alt_modifier(BareKey::Char('a')));
            assert_eq!(raw_bytes, b"\x1b[97;3u");
        },
        other => panic!("expected Kitty Alt-a, got {other:?}"),
    }
}

#[test]
fn legacy_alt_arrow_remains_one_event_at_every_split() {
    let input = b"\x1b\x1b[A";
    for split in 0..=input.len() {
        let instructions = parse_stdin_chunks(&[&input[..split], &input[split..]]);
        assert_eq!(instructions.len(), 1, "split at {split}: {instructions:?}");
        match &instructions[0] {
            InputInstruction::KeyEvent(InputEvent::Key(key), raw_bytes) => {
                assert_eq!(key.key, KeyCode::UpArrow, "split at {split}");
                assert_eq!(key.modifiers, Modifiers::ALT, "split at {split}");
                assert_eq!(raw_bytes, input, "split at {split}");
            },
            other => panic!("split at {split}: expected Alt+Up, got {other:?}"),
        }
    }
}

#[test]
fn kitty_sequence_inside_fragmented_bracketed_paste_stays_literal() {
    let start = b"\x1b[200~";
    let body = b"before\x1b[97;5uafter";
    let end_and_key = b"\x1b[201~b";
    let instructions = parse_stdin_chunks(&[start, body, end_and_key]);

    assert_eq!(instructions.len(), 2, "got {instructions:?}");
    match &instructions[0] {
        InputInstruction::KeyEvent(InputEvent::Paste(pasted), raw_bytes) => {
            assert_eq!(pasted.as_bytes(), body);
            let mut expected_raw = start.to_vec();
            expected_raw.extend_from_slice(body);
            expected_raw.extend_from_slice(b"\x1b[201~");
            assert_eq!(raw_bytes, &expected_raw);
        },
        other => panic!("expected one literal paste, got {other:?}"),
    }
    match &instructions[1] {
        InputInstruction::KeyEvent(InputEvent::Key(key), raw_bytes) => {
            assert_eq!(key.key, KeyCode::Char('b'));
            assert_eq!(raw_bytes, b"b");
        },
        other => panic!("expected plain b after paste, got {other:?}"),
    }
}

#[test]
fn recovery_discards_paste_at_every_boundary_and_chunk_split() {
    use std::collections::BTreeSet;

    let paste = b"\x1b[200~echo unsafe\r\x1b[201~";
    // Cover transitions during both markers and the body. The same reset is used
    // for connected -> offline, replacement generations, and offline -> connected.
    for boundary in 1..paste.len() {
        for split in boundary..=paste.len() {
            let mut ansi = StdinAnsiParser::new();
            let mut keyboard = InputParser::new();
            let mut buffer = Vec::new();
            let mut held = BTreeSet::new();
            let first = ansi.feed(&paste[..boundary]);
            let (instructions, _) = parse_keyboard_bytes(
                &mut keyboard,
                &mut buffer,
                &first.residue,
                true,
                true,
                false,
                &mut held,
            );
            assert!(instructions.is_empty());
            ansi.reset_for_recovery();
            keyboard = InputParser::new();
            buffer.clear();

            let middle = ansi.feed(&paste[boundary..split]);
            assert!(
                middle.residue.is_empty(),
                "boundary {boundary}, split {split}"
            );
            // Further recovery transitions and idle finalization cannot release the tail.
            ansi.reset_for_recovery();
            assert!(ansi.finalize_fast_partial().is_empty());
            assert!(ansi.finalize_force().is_empty());
            let mut last = paste[split..].to_vec();
            last.push(b'b');
            let output = ansi.feed(&last);
            assert_eq!(output.residue, b"b", "boundary {boundary}, split {split}");
            let (instructions, _) = parse_keyboard_bytes(
                &mut keyboard,
                &mut buffer,
                &output.residue,
                true,
                false,
                false,
                &mut held,
            );
            assert!(
                matches!(instructions.as_slice(), [InputInstruction::KeyEvent(InputEvent::Key(key), raw)]
                if key.key == KeyCode::Char('b') && raw == b"b"),
                "{instructions:?}"
            );
        }
    }
}

#[test]
fn idle_flush_during_paste_preserves_framing_for_recovery() {
    let end = b"\x1b[201~";
    for split in 0..end.len() {
        let mut ansi = StdinAnsiParser::new();
        ansi.feed(b"\x1b[200~echo unsafe\r");
        ansi.feed(&end[..split]);
        assert!(ansi.finalize_fast_partial().is_empty());
        assert!(ansi.finalize_force().is_empty());
        ansi.reset_for_recovery();
        let mut tail = end[split..].to_vec();
        tail.push(b'b');
        assert_eq!(ansi.feed(&tail).residue, b"b", "split {split}");
    }
}

#[test]
fn discarded_paste_cannot_inject_host_replies_or_keyboard_sequences() {
    let mut ansi = StdinAnsiParser::new();
    ansi.feed(b"\x1b[200~");
    ansi.reset_for_recovery();
    for _ in 0..32 {
        let output = ansi.feed(b"echo unsafe\r\x1b[97;5u\x1b]11;rgb:1/2/3\x1b\\\x1b[201x\x1b");
        assert!(output.residue.is_empty());
        assert!(output.replies.is_empty());
        assert!(output.nested_frames.is_empty());
        assert!(!output.has_partial_state);
    }
    // The final ESC above and this fresh ESC overlap; only the complete marker ends discard.
    assert_eq!(ansi.feed(b"\x1b[201~b").residue, b"b");
}

#[test]
fn recovery_preserves_new_input_when_an_old_prefix_is_not_a_paste() {
    for prefix in [b"\x1b".as_slice(), b"\x1b[", b"\x1b[20", b"\x1b]11;rgb:"] {
        let mut ansi = StdinAnsiParser::new();
        ansi.feed(prefix);
        ansi.reset_for_recovery();
        assert_eq!(ansi.feed(b"b").residue, b"b");
        // A fresh paste following recovery still uses ordinary paste handling.
        assert_eq!(
            ansi.feed(b"\x1b[200~safe\x1b[201~").residue,
            b"\x1b[200~safe\x1b[201~"
        );
    }
}

#[test]
fn focus_reports_are_extracted_from_the_byte_stream() {
    let (residue, focus_changes) = extract_focus_reports(b"a\x1b[Ib\x1b[Oc".to_vec());
    assert_eq!(residue, b"abc".to_vec());
    assert_eq!(focus_changes, vec![true, false]);
}

#[test]
fn a_stream_without_focus_reports_is_left_untouched() {
    let (residue, focus_changes) = extract_focus_reports(b"\x1b[A\x1b[B".to_vec());
    assert_eq!(residue, b"\x1b[A\x1b[B".to_vec());
    assert!(focus_changes.is_empty());
}

/// The bug this guards: the drain used to be handed a scratch modifier set, so an `Alt` press
/// that landed in a flushed partial was recorded nowhere - and its release then found an
/// empty set, changed nothing, and never told anyone the hold was over.
#[test]
fn a_modifier_press_drained_from_a_partial_is_still_held_afterwards() {
    use std::collections::BTreeSet;
    use yinz_types::keys::KeyModifier;
    let mut input_parser = InputParser::new();
    let mut termwiz_buffer = vec![];
    let mut held_modifiers = BTreeSet::new();

    let drained = drain_partial_to_keyboard(
        &mut input_parser,
        &mut termwiz_buffer,
        b"\x1b[57443;1u".to_vec(),
        true,
        true,
        &mut held_modifiers,
    );
    assert_eq!(
        held_modifiers,
        [KeyModifier::Alt].into_iter().collect::<BTreeSet<_>>()
    );
    match drained.as_slice() {
        [InputInstruction::ModifiersHeldChanged(held)] => {
            assert_eq!(held, &held_modifiers)
        },
        other => panic!("expected the drained press to announce the hold, got {other:?}"),
    }

    // The release arrives on an ordinary read, and can only be recognised as one because the
    // press before it went into the same set.
    let (instructions, _) = parse_keyboard_bytes(
        &mut input_parser,
        &mut termwiz_buffer,
        b"\x1b[57449;1:3u",
        true,
        false,
        true,
        &mut held_modifiers,
    );
    assert!(held_modifiers.is_empty());
    match instructions.as_slice() {
        [InputInstruction::ModifiersHeldChanged(held)] => assert!(held.is_empty()),
        other => panic!("expected the release to announce an empty hold, got {other:?}"),
    }
}

/// The bug this guards: alt-tab away on `Alt` and the release lands in the other window, so
/// the stale `Alt` made the next real press a no-op and the hold was never heard of again.
#[test]
fn a_focus_change_forgets_what_was_held() {
    use std::collections::BTreeSet;
    use yinz_types::keys::KeyModifier;
    let mut held: BTreeSet<KeyModifier> = [KeyModifier::Alt].into_iter().collect();
    match forget_held_modifiers(&mut held) {
        Some(InputInstruction::ModifiersHeldChanged(announced)) => {
            assert!(announced.is_empty())
        },
        other => panic!("expected an empty hold to be announced, got {other:?}"),
    }
    assert!(held.is_empty());
}

#[test]
fn a_focus_change_with_nothing_held_says_nothing() {
    use std::collections::BTreeSet;
    use yinz_types::keys::KeyModifier;
    let mut held: BTreeSet<KeyModifier> = BTreeSet::new();
    assert!(forget_held_modifiers(&mut held).is_none());
    assert!(held.is_empty());
}

#[test]
fn pixel_size_query_probes_text_area_and_character_cell() {
    assert_eq!(PIXEL_SIZE_QUERY, "\u{1b}[14t\u{1b}[16t");
}

#[test]
fn startup_query_has_no_palette_register_loop() {
    let query = build_startup_query_string(true);
    assert_eq!(
            query,
            "\u{1b}[14t\u{1b}[16t\u{1b}]11;?\u{1b}\u{5c}\u{1b}]10;?\u{1b}\u{5c}\u{1b}[?2026$p\u{1b}_Ga=q,i=31,s=1,v=1,t=d,f=24;AAAA\u{1b}\u{5c}\u{1b}[c"
        );
    assert!(
        !query.contains("\u{1b}]4;"),
        "startup query must not contain OSC 4 palette-register probes: {:?}",
        query
    );
}

#[test]
fn startup_query_contains_kitty_probe_before_barrier() {
    let query = build_startup_query_string(true);
    assert!(query.contains("\u{1b}_Ga=q,i=31,s=1,v=1,t=d,f=24;AAAA\u{1b}\u{5c}"));
    let probe_pos = query.find("\u{1b}_Ga=q,i=31,").unwrap();
    let barrier_pos = query.find("\u{1b}[c").unwrap();
    assert!(probe_pos < barrier_pos);
}

#[test]
fn startup_query_omits_kitty_probe_when_the_protocol_is_disabled() {
    let query = build_startup_query_string(false);
    assert!(!query.contains("\u{1b}_G"));
    assert!(query.ends_with("\u{1b}[c"));
    assert!(query.starts_with("\u{1b}[14t\u{1b}[16t"));
}
