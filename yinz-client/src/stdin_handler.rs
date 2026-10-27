use crate::keyboard_parser::{KeyEventKind, KittyKeyboardParser, KittyParseOutcome};
use crate::os_input_output::ClientOsApi;
use crate::stdin_ansi_parser::{HostReply, PendingPartial, StdinAnsiParser};
use crate::InputInstruction;
use std::collections::BTreeSet;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

const LONE_ESC_FLUSH_INTERVAL: Duration = Duration::from_millis(50);
const PARTIAL_REPLY_FLUSH_GUARD: Duration = Duration::from_millis(1000);
const STARTUP_KITTY_PROBE_TIMEOUT: Duration =
    Duration::from_millis(crate::stdin_ansi_parser::CLIENT_KITTY_PROBE_TIMEOUT_MS);
use yinz_key_codec::vendored::termwiz::input::InputParser;
use yinz_runtime::channels::SenderWithContext;
use yinz_types::keys::KeyModifier;

struct StampedInput {
    sender: SenderWithContext<InputInstruction>,
    stamp: Option<(u64, bool)>,
}
impl StampedInput {
    fn send(
        &self,
        instruction: InputInstruction,
    ) -> Result<
        (),
        yinz_runtime::channels::SendError<(
            InputInstruction,
            yinz_types::diagnostics::ErrorContext,
        )>,
    > {
        self.sender.send(match self.stamp {
            Some(stamp) => InputInstruction::FromGeneration(stamp, Box::new(instruction)),
            None => instruction,
        })
    }
}

pub(crate) fn stdin_loop(
    mut os_input: Box<dyn ClientOsApi>,
    send_input_instructions: SenderWithContext<InputInstruction>,
    stdin_ansi_parser: Arc<Mutex<StdinAnsiParser>>,
    explicitly_disable_kitty_keyboard_protocol: bool,
    support_kitty_graphics_protocol: bool,
    report_modifier_keys: bool,
) {
    let recovery = os_input.remote_recovery();
    let mut send_input_instructions = StampedInput {
        sender: send_input_instructions,
        stamp: recovery.as_ref().map(|control| control.stamp()),
    };
    // Send the startup host query string so the host terminal replies
    // with its live pixel dimensions, fg/bg, sync-output support, and
    // palette registers. These replies will be classified by the
    // continuous parser as they arrive and routed via `InputInstruction::
    // AnsiStdinInstructions` — no cache, no loading gate. The kitty probe is the one reply with
    // a deadline, because it is the one whose absence is itself an answer.
    let mut kitty_probe_deadline: Option<Instant> = None;
    {
        let query_string = build_startup_query_string(support_kitty_graphics_protocol);
        // `write_all` + `flush`, not `write`: the query holds no newline, and stdout is a
        // `LineWriter`, so a bare write would sit in the buffer until some other thread flushed
        // it - and a short write would cut a query in half mid-escape-sequence.
        let mut stdout = os_input.get_stdout_writer();
        if let Err(e) = stdout
            .write_all(query_string.as_bytes())
            .and_then(|_| stdout.flush())
        {
            log::error!("failed to write the startup host query: {e}");
        }
        if support_kitty_graphics_protocol {
            stdin_ansi_parser.lock().unwrap().expect_kitty_probe_reply();
            // A host that answers neither the probe nor the Primary-DA barrier would otherwise
            // leave the capability unknown for the life of the session.
            kitty_probe_deadline = Some(Instant::now() + STARTUP_KITTY_PROBE_TIMEOUT);
            log::info!("kitty graphics: probing the host terminal");
        } else {
            log::info!("kitty graphics: disabled for this client, not probing");
            let _ = send_input_instructions.send(InputInstruction::AnsiStdinInstructions(vec![
                HostReply::KittyGraphicsSupport(false),
            ]));
        }
    }

    // Byte reader + termwiz/kitty parser path.
    let mut input_parser = InputParser::new();
    let mut termwiz_buffer = vec![];
    // Which modifier keys the host says are physically down. Only ever non-empty on a terminal
    // asked for `REPORT_ALL_KEYS_AS_ESCAPE_CODES`; everywhere else no modifier key is ever
    // reported on its own and this stays empty for the life of the client.
    let mut held_modifiers: BTreeSet<KeyModifier> = BTreeSet::new();
    let (stdin_tx, stdin_rx) = mpsc::sync_channel(32);
    let _stdin_pump = std::thread::Builder::new()
        .name("stdin_pump".to_string())
        .spawn({
            let recovery = recovery.clone();
            move || loop {
                match os_input.read_from_stdin() {
                    Ok(buf) => {
                        if stdin_tx
                            .send(Ok((buf, recovery.as_ref().map(|control| control.stamp()))))
                            .is_err()
                        {
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
    // When the pending partial is allowed to be drained to the keyboard. Kept apart from the
    // probe's deadline: waking for the probe must never cut this window short.
    let mut finalize_at: Option<Instant> = None;
    let mut reply_in_progress_since: Option<Instant> = None;
    'stdin: loop {
        if kitty_probe_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            kitty_probe_deadline = None;
            let timed_out = stdin_ansi_parser
                .lock()
                .unwrap()
                .resolve_kitty_probe_by_timeout();
            if let Some(reply) = timed_out {
                log::info!(
                    "kitty graphics: the host answered neither the probe nor the barrier within \
                     {STARTUP_KITTY_PROBE_TIMEOUT:?}, assuming no support"
                );
                let _ = send_input_instructions
                    .send(InputInstruction::AnsiStdinInstructions(vec![reply]));
            }
        }
        // The loop would otherwise park in `recv()` forever on an idle client, and the probe's
        // deadline would never be looked at.
        let wait = next_wakeup(Instant::now(), finalize_at, kitty_probe_deadline);
        let received = match wait {
            Some(wait) => stdin_rx.recv_timeout(wait),
            None => stdin_rx
                .recv()
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected),
        };
        match received {
            Ok(result) => {
                match result {
                    Ok((buf, stamp)) => {
                        if send_input_instructions.stamp != stamp {
                            stdin_ansi_parser.lock().unwrap().reset_for_recovery();
                            input_parser = InputParser::new();
                            termwiz_buffer.clear();
                            held_modifiers.clear();
                            finalize_at = None;
                            reply_in_progress_since = None;
                        }
                        send_input_instructions.stamp = stamp;
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
                            // Before the focus news, so a consumer sees the hold end rather than
                            // outliving the window it was held in.
                            if let Some(forgotten) = forget_held_modifiers(&mut held_modifiers) {
                                let _ = send_input_instructions.send(forgotten);
                            }
                            let _ = send_input_instructions
                                .send(InputInstruction::HostTerminalFocusChanged(focused));
                        }
                        if residue.is_empty() {
                            schedule_finalization(
                                &stdin_ansi_parser,
                                false,
                                &mut finalize_at,
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
                            report_modifier_keys,
                            &mut held_modifiers,
                        );
                        for instruction in instructions {
                            if send_input_instructions.send(instruction).is_err() {
                                break 'stdin;
                            }
                        }

                        schedule_finalization(
                            &stdin_ansi_parser,
                            fed_termwiz,
                            &mut finalize_at,
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
                // The probe deadline can wake this arm too, and it must not touch the escape
                // window: flushing here with nothing pending - or with a partial whose 50ms
                // window has not run out yet - would turn a lone ESC into a premature Escape
                // key, or split a host reply the rest of which is still in flight.
                if finalize_at.is_none_or(|at| Instant::now() < at) {
                    continue;
                }
                let pending = stdin_ansi_parser.lock().unwrap().pending_partial();
                if pending.uses_reply_flush_guard() {
                    let elapsed = reply_in_progress_since
                        .map(|since| since.elapsed())
                        .unwrap_or_default();
                    if elapsed >= PARTIAL_REPLY_FLUSH_GUARD {
                        let drained = stdin_ansi_parser.lock().unwrap().finalize_force();
                        for instruction in drain_partial_to_keyboard(
                            &mut input_parser,
                            &mut termwiz_buffer,
                            drained,
                            !explicitly_disable_kitty_keyboard_protocol,
                            report_modifier_keys,
                            &mut held_modifiers,
                        ) {
                            if send_input_instructions.send(instruction).is_err() {
                                break 'stdin;
                            }
                        }
                        finalize_at = None;
                        reply_in_progress_since = None;
                    } else {
                        // Without a fresh window `next_wakeup` would return zero and this
                        // branch would spin for the whole `PARTIAL_REPLY_FLUSH_GUARD`.
                        finalize_at = Some(Instant::now() + LONE_ESC_FLUSH_INTERVAL);
                    }
                } else {
                    let drained = stdin_ansi_parser.lock().unwrap().finalize_fast_partial();
                    for instruction in drain_partial_to_keyboard(
                        &mut input_parser,
                        &mut termwiz_buffer,
                        drained,
                        !explicitly_disable_kitty_keyboard_protocol,
                        report_modifier_keys,
                        &mut held_modifiers,
                    ) {
                        if send_input_instructions.send(instruction).is_err() {
                            break 'stdin;
                        }
                    }
                    finalize_at = None;
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

/// The next moment the loop has to wake up for: whichever of the two pending deadlines comes
/// first, or `None` when neither is set and the loop may park indefinitely. They are kept
/// separate because they mean different things - the probe's deadline only needs looking at,
/// while `finalize_at` is the moment a partial may be drained to the keyboard.
fn next_wakeup(
    now: Instant,
    finalize_at: Option<Instant>,
    kitty_probe_deadline: Option<Instant>,
) -> Option<Duration> {
    [finalize_at, kitty_probe_deadline]
        .into_iter()
        .flatten()
        .min()
        .map(|deadline| deadline.saturating_duration_since(now))
}

fn schedule_finalization(
    stdin_ansi_parser: &Arc<Mutex<StdinAnsiParser>>,
    fed_termwiz: bool,
    finalize_at: &mut Option<Instant>,
    reply_in_progress_since: &mut Option<Instant>,
) {
    let pending = stdin_ansi_parser.lock().unwrap().pending_partial();
    // Every read chunk restarts the window, as it did when the wait was a bare
    // `recv_timeout(LONE_ESC_FLUSH_INTERVAL)`.
    if finalize_at.is_some() || fed_termwiz || pending != PendingPartial::None {
        *finalize_at = Some(Instant::now() + LONE_ESC_FLUSH_INTERVAL);
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
    report_modifier_keys: bool,
    held_modifiers: &mut BTreeSet<KeyModifier>,
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
            let outcome = termwiz_buffer
                .is_empty()
                .then(|| KittyKeyboardParser::new().feed(candidate));
            let kitty_key = match outcome {
                // A modifier key on its own is never forwarded anywhere: it is state about the
                // keyboard, not something to type. Only a change is announced, so a key held
                // down through its auto-repeat does not produce a message per repeat.
                Some(KittyParseOutcome::ModifierKey { modifier, event }) => {
                    let changed = match event {
                        KeyEventKind::Release => held_modifiers.remove(&modifier),
                        KeyEventKind::Press | KeyEventKind::Repeat => {
                            held_modifiers.insert(modifier)
                        },
                    };
                    if changed {
                        instructions.push(InputInstruction::ModifiersHeldChanged(
                            held_modifiers.clone(),
                        ));
                    }
                    cursor = end;
                    continue;
                },
                // A release is the other half of a press we already acted on. Forwarding it
                // would type the key a second time.
                Some(KittyParseOutcome::Complete(_, KeyEventKind::Release)) => {
                    cursor = end;
                    continue;
                },
                Some(KittyParseOutcome::Complete(key, _)) => Some(key),
                Some(KittyParseOutcome::Incomplete) | Some(KittyParseOutcome::NoMatch) | None => {
                    None
                },
            };
            if let Some(key) = kitty_key {
                // The raw bytes ride along so a pane that asked for kitty mode itself can be
                // written the host's own sequence. Panes only ever get flag 1
                // (`yinz_key_codec::keys::serialize_kitty`), so under the extended flags the host's
                // bytes are the wrong shape for them - they would carry event types and
                // alternate keys nothing downstream asked for. Re-serialise instead.
                // A key `serialize_kitty` cannot spell keeps the host's bytes: they are still
                // a better guess than nothing, and this is only the passthrough copy.
                let raw_bytes = match report_modifier_keys {
                    true => yinz_key_codec::keys::serialize_kitty(&key)
                        .map(String::into_bytes)
                        .unwrap_or_else(|| candidate.to_vec()),
                    false => candidate.to_vec(),
                };
                instructions.push(InputInstruction::KeyWithModifierEvent(key, raw_bytes, true));
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
    let mut events = vec![];
    input_parser.parse_with_consumed(
        bytes,
        |input_event, consumed| {
            events.push((input_event, consumed));
        },
        maybe_more,
    );
    for (input_event, consumed) in events {
        let take = consumed.min(termwiz_buffer.len());
        let raw_bytes: Vec<u8> = termwiz_buffer.drain(..take).collect();
        if let Some(input_event) = input_event {
            instructions.push(InputInstruction::KeyEvent(input_event, raw_bytes));
        }
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

/// Parse a flushed partial and return what it produced.
///
/// Drained bytes are keyboard bytes like any others, and a modifier key's own CSI can be among
/// them, so this takes the loop's `held_modifiers` rather than a scratch set. Given a set of its
/// own, a press recorded here would be lost and its release would find nothing to remove, so no
/// release event would ever be emitted.
fn drain_partial_to_keyboard(
    input_parser: &mut InputParser,
    termwiz_buffer: &mut Vec<u8>,
    drained: Vec<u8>,
    kitty_enabled: bool,
    report_modifier_keys: bool,
    held_modifiers: &mut BTreeSet<KeyModifier>,
) -> Vec<InputInstruction> {
    let (instructions, _) = parse_keyboard_bytes(
        input_parser,
        termwiz_buffer,
        &drained,
        kitty_enabled,
        false,
        report_modifier_keys,
        held_modifiers,
    );
    instructions
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
    // Primary DA provisionally reports no support when the probe is still
    // unanswered; a delayed graphics reply can still confirm support
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

/// Drop everything the client believes is held down, announcing the change if there was one.
///
/// The host takes the keyboard away without replaying the releases of whatever was down when it
/// did - alt-tab away on `Alt` and the release lands in the other window. Left standing, the
/// stale `Alt` makes the next real press a no-op, because it is already in the set, and nothing
/// downstream ever hears about the hold again.
///
/// `None` when there was nothing held, so a client that never sees a modifier key does not put a
/// message on the wire for every focus flicker.
fn forget_held_modifiers(held: &mut BTreeSet<KeyModifier>) -> Option<InputInstruction> {
    if held.is_empty() {
        return None;
    }
    held.clear();
    Some(InputInstruction::ModifiersHeldChanged(BTreeSet::new()))
}

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
#[path = "unit/stdin_handler_recovery_tests.rs"]
mod recovery_tests;

#[cfg(test)]
#[path = "unit/stdin_handler_tests.rs"]
mod tests;
