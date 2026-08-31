use crate::keyboard_parser::{KittyKeyboardParser, KittyParseOutcome};
use crate::os_input_output::ClientOsApi;
use crate::stdin_ansi_parser::{HostReply, PendingPartial, StdinAnsiParser};
use crate::InputInstruction;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

const LONE_ESC_FLUSH_INTERVAL: Duration = Duration::from_millis(50);
const PARTIAL_REPLY_FLUSH_GUARD: Duration = Duration::from_millis(1000);
use zellij_utils::{
    channels::SenderWithContext,
    vendored::termwiz::input::{InputEvent, InputParser},
};

pub(crate) fn stdin_loop(
    mut os_input: Box<dyn ClientOsApi>,
    send_input_instructions: SenderWithContext<InputInstruction>,
    stdin_ansi_parser: Arc<Mutex<StdinAnsiParser>>,
    explicitly_disable_kitty_keyboard_protocol: bool,
    support_kitty_graphics_protocol: bool,
) {
    // Send the startup host query string so the host terminal replies
    // with its live pixel dimensions, fg/bg, sync-output support, and
    // palette registers. These replies will be classified by the
    // continuous parser as they arrive and routed via `InputInstruction::
    // AnsiStdinInstructions` — no deadline, no cache, no loading gate.
    {
        let query_string = build_startup_query_string(support_kitty_graphics_protocol);
        let _ = os_input
            .get_stdout_writer()
            .write(query_string.as_bytes())
            .unwrap();
        if support_kitty_graphics_protocol {
            stdin_ansi_parser.lock().unwrap().expect_kitty_probe_reply();
        } else {
            let _ = send_input_instructions.send(InputInstruction::AnsiStdinInstructions(vec![
                HostReply::KittyGraphicsSupport(false),
            ]));
        }
    }

    // Byte reader + termwiz/kitty parser path.
    let mut input_parser = InputParser::new();
    let mut termwiz_buffer = vec![];
    let (stdin_tx, stdin_rx) = mpsc::sync_channel(32);
    let _stdin_pump = std::thread::Builder::new()
        .name("stdin_pump".to_string())
        .spawn({
            move || loop {
                match os_input.read_from_stdin() {
                    Ok(buf) => {
                        if stdin_tx.send(Ok(buf)).is_err() {
                            break; // receiver dropped
                        }
                    },
                    Err(e) => {
                        let _ = stdin_tx.send(Err(e));
                        break;
                    },
                }
            }
        });
    let mut needs_finalization = false;
    let mut reply_in_progress_since: Option<Instant> = None;
    'stdin: loop {
        match if needs_finalization {
            stdin_rx.recv_timeout(LONE_ESC_FLUSH_INTERVAL)
        } else {
            stdin_rx
                .recv()
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
        } {
            Ok(result) => {
                match result {
                    Ok(buf) => {
                        // Strip + classify any host-reply sequences
                        // continuously. The residue is the byte stream
                        // the keyboard parser should see.
                        let parse_output = {
                            let mut p = stdin_ansi_parser.lock().unwrap();
                            p.feed(&buf)
                        };
                        if !parse_output.replies.is_empty() {
                            let _ = send_input_instructions.send(
                                InputInstruction::AnsiStdinInstructions(parse_output.replies),
                            );
                        }
                        // Either kind of forwarding window may have closed on
                        // this chunk; both relay the same completion.
                        for (token, reply_bytes) in parse_output
                            .completed_forward
                            .into_iter()
                            .chain(parse_output.completed_clipboard_forward)
                        {
                            let _ = send_input_instructions.send(
                                InputInstruction::ForwardedReplyFromHostComplete {
                                    token,
                                    reply_bytes,
                                },
                            );
                        }
                        for payload in parse_output.desktop_notifications {
                            let _ = send_input_instructions
                                .send(InputInstruction::DesktopNotificationResponse(payload));
                        }
                        for payload_bytes in parse_output.nested_frames {
                            let _ = send_input_instructions
                                .send(InputInstruction::NestedSessionFrameFromHost(payload_bytes));
                        }
                        let (residue, focus_changes) = extract_focus_reports(parse_output.residue);
                        for focused in focus_changes {
                            let _ = send_input_instructions
                                .send(InputInstruction::HostTerminalFocusChanged(focused));
                        }
                        if residue.is_empty() {
                            schedule_finalization(
                                &stdin_ansi_parser,
                                false,
                                &mut needs_finalization,
                                &mut reply_in_progress_since,
                            );
                            continue;
                        }
                        let (instructions, fed_termwiz) = parse_keyboard_bytes(
                            &mut input_parser,
                            &mut termwiz_buffer,
                            &residue,
                            !explicitly_disable_kitty_keyboard_protocol,
                            true,
                        );
                        for instruction in instructions {
                            if send_input_instructions.send(instruction).is_err() {
                                break 'stdin;
                            }
                        }

                        schedule_finalization(
                            &stdin_ansi_parser,
                            fed_termwiz,
                            &mut needs_finalization,
                            &mut reply_in_progress_since,
                        );
                    },
                    Err(e) => {
                        if e == "Session ended" {
                            log::debug!("Switched sessions, signing this thread off...");
                        } else {
                            log::error!("Failed to read from STDIN: {}", e);
                        }
                        let _ = send_input_instructions.send(InputInstruction::Exit);
                        break;
                    },
                }
            },
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let pending = stdin_ansi_parser.lock().unwrap().pending_partial();
                if pending.uses_reply_flush_guard() {
                    let elapsed = reply_in_progress_since
                        .map(|since| since.elapsed())
                        .unwrap_or_default();
                    if elapsed >= PARTIAL_REPLY_FLUSH_GUARD {
                        let drained = stdin_ansi_parser.lock().unwrap().finalize_force();
                        drain_partial_to_keyboard(
                            &mut input_parser,
                            &mut termwiz_buffer,
                            send_input_instructions.clone(),
                            drained,
                            !explicitly_disable_kitty_keyboard_protocol,
                        );
                        needs_finalization = false;
                        reply_in_progress_since = None;
                    } else {
                        needs_finalization = true;
                    }
                } else {
                    let drained = stdin_ansi_parser.lock().unwrap().finalize_fast_partial();
                    drain_partial_to_keyboard(
                        &mut input_parser,
                        &mut termwiz_buffer,
                        send_input_instructions.clone(),
                        drained,
                        !explicitly_disable_kitty_keyboard_protocol,
                    );
                    needs_finalization = false;
                    reply_in_progress_since = None;
                }
            },
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                log::debug!("STDIN pump disconnected");
                let _ = send_input_instructions.send(InputInstruction::Exit);
                break;
            },
        }
    }
}

fn schedule_finalization(
    stdin_ansi_parser: &Arc<Mutex<StdinAnsiParser>>,
    fed_termwiz: bool,
    needs_finalization: &mut bool,
    reply_in_progress_since: &mut Option<Instant>,
) {
    let pending = stdin_ansi_parser.lock().unwrap().pending_partial();
    if fed_termwiz || pending != PendingPartial::None {
        *needs_finalization = true;
    }
    if pending.uses_reply_flush_guard() {
        if reply_in_progress_since.is_none() {
            *reply_in_progress_since = Some(Instant::now());
        }
    } else {
        *reply_in_progress_since = None;
    }
}

/// Parse keyboard residue without letting the Kitty and termwiz parsers own
/// the same bytes. `StdinAnsiParser` has already reassembled incomplete CSI
/// sequences, so each framed CSI candidate can be classified independently.
fn parse_keyboard_bytes(
    input_parser: &mut InputParser,
    termwiz_buffer: &mut Vec<u8>,
    bytes: &[u8],
    kitty_enabled: bool,
    maybe_more: bool,
) -> (Vec<InputInstruction>, bool) {
    let mut instructions = vec![];
    let mut fed_termwiz = false;
    let mut cursor = 0;

    if kitty_enabled {
        while let Some((start, end)) = next_complete_csi(&bytes[cursor..]) {
            let start = cursor + start;
            let end = cursor + end;
            if cursor < start {
                feed_termwiz(
                    input_parser,
                    termwiz_buffer,
                    &bytes[cursor..start],
                    true,
                    &mut instructions,
                );
                fed_termwiz = true;
            }

            let candidate = &bytes[start..end];
            // A previous ambiguous key or an open bracketed paste owns all
            // bytes until termwiz produces its event. In particular, do not
            // finalize a parked ESC before its following legacy CSI.
            let kitty_key = termwiz_buffer
                .is_empty()
                .then(|| KittyKeyboardParser::new().feed(candidate))
                .and_then(|outcome| match outcome {
                    KittyParseOutcome::Complete(key) => Some(key),
                    KittyParseOutcome::Incomplete | KittyParseOutcome::NoMatch => None,
                });
            if let Some(key) = kitty_key {
                instructions.push(InputInstruction::KeyWithModifierEvent(
                    key,
                    candidate.to_vec(),
                    true,
                ));
            } else {
                feed_termwiz(
                    input_parser,
                    termwiz_buffer,
                    candidate,
                    true,
                    &mut instructions,
                );
                fed_termwiz = true;
            }
            cursor = end;
        }
    }

    if cursor < bytes.len() {
        feed_termwiz(
            input_parser,
            termwiz_buffer,
            &bytes[cursor..],
            true,
            &mut instructions,
        );
        fed_termwiz = true;
    }
    if !maybe_more {
        feed_termwiz(input_parser, termwiz_buffer, &[], false, &mut instructions);
    }
    (instructions, fed_termwiz)
}

fn feed_termwiz(
    input_parser: &mut InputParser,
    termwiz_buffer: &mut Vec<u8>,
    bytes: &[u8],
    maybe_more: bool,
    instructions: &mut Vec<InputInstruction>,
) {
    termwiz_buffer.extend_from_slice(bytes);
    let mut events: Vec<(InputEvent, usize)> = vec![];
    input_parser.parse_with_consumed(
        bytes,
        |input_event: InputEvent, consumed: usize| {
            events.push((input_event, consumed));
        },
        maybe_more,
    );
    for (input_event, consumed) in events {
        let take = consumed.min(termwiz_buffer.len());
        let raw_bytes: Vec<u8> = termwiz_buffer.drain(..take).collect();
        instructions.push(InputInstruction::KeyEvent(input_event, raw_bytes));
    }
}

/// Find the next structurally complete 7-bit CSI sequence in `bytes` and
/// return its start/end offsets. Invalid or incomplete candidates are skipped
/// so a later complete candidate in the same read can still be recognized.
fn next_complete_csi(bytes: &[u8]) -> Option<(usize, usize)> {
    let mut search_from = 0;
    while search_from + 1 < bytes.len() {
        let relative_start = bytes[search_from..]
            .windows(2)
            .position(|window| window == b"\x1b[")?;
        let start = search_from + relative_start;
        if let Some(len) = complete_csi_len(&bytes[start..]) {
            return Some((start, start + len));
        }
        search_from = start + 1;
    }
    None
}

fn complete_csi_len(bytes: &[u8]) -> Option<usize> {
    if !bytes.starts_with(b"\x1b[") {
        return None;
    }
    for (offset, byte) in bytes[2..].iter().enumerate() {
        match byte {
            0x20..=0x3f => {},
            0x40..=0x7e => return Some(offset + 3),
            _ => return None,
        }
    }
    None
}

fn drain_partial_to_keyboard(
    input_parser: &mut InputParser,
    termwiz_buffer: &mut Vec<u8>,
    send_input_instructions: SenderWithContext<InputInstruction>,
    drained: Vec<u8>,
    kitty_enabled: bool,
) {
    let (instructions, _) =
        parse_keyboard_bytes(input_parser, termwiz_buffer, &drained, kitty_enabled, false);
    for instruction in instructions {
        send_input_instructions.send(instruction).unwrap();
    }
}

/// Build the fire-and-forget host-query batch sent at client startup.
/// The host's replies refine `Screen`'s cached state asynchronously as
/// they arrive; the UI does not block on them.
fn build_startup_query_string(support_kitty_graphics_protocol: bool) -> String {
    // <ESC>[14t => get text area size in pixels,
    // <ESC>[16t => get character cell size in pixels
    // <ESC>]11;?<ESC>\ => get background color
    // <ESC>]10;?<ESC>\ => get foreground color
    // <ESC>[?2026$p => get synchronised output mode
    // <ESC>_Ga=q,...<ESC>\ => probe kitty graphics support (omitted when the
    // protocol is disabled), answered by capable terminals only; the trailing
    // Primary DA is the barrier that resolves the probe negatively when it
    // goes unanswered
    let kitty_graphics_probe = if support_kitty_graphics_protocol {
        "\u{1b}_Ga=q,i=31,s=1,v=1,t=d,f=24;AAAA\u{1b}\u{5c}"
    } else {
        ""
    };
    format!(
        "{}\u{1b}]11;?\u{1b}\u{5c}\u{1b}]10;?\u{1b}\u{5c}\u{1b}[?2026$p{}\u{1b}[c",
        PIXEL_SIZE_QUERY, kitty_graphics_probe
    )
}

pub(crate) const PIXEL_SIZE_QUERY: &str = "\u{1b}[14t\u{1b}[16t";

/// Strips the host's DECSET 1004 focus reports (`CSI I` / `CSI O`) out of the
/// keyboard residue and returns them as focus transitions, in order.
fn extract_focus_reports(residue: Vec<u8>) -> (Vec<u8>, Vec<bool>) {
    const FOCUS_GAINED: &[u8] = b"\x1b[I";
    const FOCUS_LOST: &[u8] = b"\x1b[O";
    if residue.len() < FOCUS_GAINED.len() {
        return (residue, vec![]);
    }
    let mut remaining = Vec::with_capacity(residue.len());
    let mut focus_changes = vec![];
    let mut index = 0;
    while index < residue.len() {
        let rest = &residue[index..];
        if rest.starts_with(FOCUS_GAINED) {
            focus_changes.push(true);
            index += FOCUS_GAINED.len();
        } else if rest.starts_with(FOCUS_LOST) {
            focus_changes.push(false);
            index += FOCUS_LOST.len();
        } else {
            remaining.push(residue[index]);
            index += 1;
        }
    }
    (remaining, focus_changes)
}

#[cfg(test)]
mod tests {
    use super::{
        build_startup_query_string, extract_focus_reports, parse_keyboard_bytes, PIXEL_SIZE_QUERY,
    };
    use crate::{stdin_ansi_parser::StdinAnsiParser, InputInstruction};
    use zellij_utils::{
        data::BareKey,
        vendored::termwiz::input::{InputEvent, InputParser, KeyCode, Modifiers},
    };

    fn parse_stdin_chunks(chunks: &[&[u8]]) -> Vec<InputInstruction> {
        let mut ansi_parser = StdinAnsiParser::new();
        let mut input_parser = InputParser::new();
        let mut termwiz_buffer = vec![];
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
            );
            instructions.append(&mut parsed);
        }
        let (mut flushed, _) =
            parse_keyboard_bytes(&mut input_parser, &mut termwiz_buffer, &[], true, false);
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
                assert!(key.is_key_with_shift_modifier(BareKey::Char('b')));
                assert_eq!(raw_bytes, b"\x1b[98;2u");
            },
            other => panic!("expected Kitty Shift-b, got {other:?}"),
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
}
