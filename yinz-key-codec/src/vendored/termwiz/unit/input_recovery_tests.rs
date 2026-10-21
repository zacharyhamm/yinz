use super::*;

const OSC: &[u8] = b"\x1b]11;rgb:1111/2222/3333\x1b\\";
const DA: &[u8] = b"\x1b[?62;1;6c";
const TRIGGERS: &[&[u8]] = &[
    b"\x1b[?1u",
    b"\x1b[?6;1R",
    b"\x1b[<u",
    b"\x1b[<0;1M",
    b"\x1b[<0;;1m",
    b"\x1b[<999;1;1M",
    b"\x1b]11;interrupted\x1b[?1u",
    b"\x1b]11;interrupted",
    b"\x1b[?1\x01",
    b"\x1b[<0;\x7f",
    b"\x1b[123\x01",
    b"\x1b[?1\x1b[<u",
];

fn key(key: KeyCode) -> InputEvent {
    InputEvent::Key(KeyEvent {
        key,
        modifiers: Modifiers::NONE,
    })
}

fn osc() -> InputEvent {
    InputEvent::OperatingSystemCommand(b"11;rgb:1111/2222/3333".to_vec())
}

fn da() -> InputEvent {
    InputEvent::DeviceControlReply {
        intermediates: vec![],
        params: b"?62;1;6".to_vec(),
        final_byte: b'c',
        raw: DA.to_vec(),
    }
}

/// Attribute raw bytes as the stdin handler does, including bytes held across reads.
fn parse_chunks(chunks: &[&[u8]], maybe_more: bool) -> Vec<(InputEvent, Vec<u8>)> {
    let mut parser = InputParser::new();
    let mut pending = Vec::new();
    let mut events = Vec::new();
    for chunk in chunks {
        pending.extend_from_slice(chunk);
        parser.parse_with_consumed(
            chunk,
            |event, consumed| {
                let raw = pending.drain(..consumed).collect();
                if let Some(event) = event {
                    events.push((event, raw));
                }
            },
            maybe_more,
        );
    }
    assert!(parser.buf.is_empty(), "buffered: {:?}", parser.buf);
    assert!(pending.is_empty(), "unattributed: {pending:?}");
    events
}

#[test]
fn legacy_alt_shift_insert_before_backspace_keeps_its_modifier_and_bytes() {
    let bytes = b"\x1b\x1b[2$\x7f";
    for maybe_more in [false, true] {
        let splits: Vec<_> = if maybe_more {
            (0..=bytes.len()).collect()
        } else {
            vec![0]
        };
        for split in splits {
            let events = parse_chunks(&[&bytes[..split], &bytes[split..]], maybe_more);
            assert_eq!(
                events,
                vec![
                    (
                        InputEvent::Key(KeyEvent {
                            key: KeyCode::Insert,
                            modifiers: Modifiers::ALT | Modifiers::SHIFT,
                        }),
                        b"\x1b\x1b[2$".to_vec(),
                    ),
                    (key(KeyCode::Backspace), b"\x7f".to_vec()),
                ],
                "split at {split}, maybe_more={maybe_more}",
            );
        }
    }
}

#[test]
fn recovery_keeps_later_keys_and_replies_across_reads() {
    for trigger in TRIGGERS {
        let bytes = [*trigger, b"z", OSC, DA].concat();
        for split in 0..=bytes.len() {
            let events = parse_chunks(&[&bytes[..split], &bytes[split..]], true);
            assert_eq!(events[events.len() - 3].0, key(KeyCode::Char('z')));
            assert_eq!(events[events.len() - 2], (osc(), OSC.to_vec()));
            assert_eq!(events[events.len() - 1], (da(), DA.to_vec()));
            assert!(events[..events.len() - 3]
                .iter()
                .all(|(event, _)| matches!(event, InputEvent::Key(_))));
        }
        let chunks: Vec<_> = bytes.chunks(1).collect();
        let events = parse_chunks(&chunks, true);
        assert_eq!(events[events.len() - 3].0, key(KeyCode::Char('z')));
        assert_eq!(events[events.len() - 2], (osc(), OSC.to_vec()));
        assert_eq!(events[events.len() - 1], (da(), DA.to_vec()));
    }
}

#[test]
fn discarded_csi_reports_consumption_without_attaching_bytes_to_later_keys() {
    for trigger in TRIGGERS[..6]
        .iter()
        .copied()
        .chain([b"\x1b[97;1:1u".as_slice()])
    {
        for split in 0..=trigger.len() {
            let mut parser = InputParser::new();
            let mut events = Vec::new();
            parser.parse_with_consumed(&trigger[..split], |event, n| events.push((event, n)), true);
            if split < trigger.len() {
                assert!(events.is_empty());
            }
            parser.parse_with_consumed(&trigger[split..], |event, n| events.push((event, n)), true);
            assert_eq!(events, vec![(None, trigger.len())]);
            assert!(parser.buf.is_empty());
            events.clear();
            parser.parse_with_consumed(&[], |event, n| events.push((event, n)), false);
            assert!(events.is_empty());
            parser.parse_with_consumed(b"z", |event, n| events.push((event, n)), true);
            assert_eq!(events, vec![(Some(key(KeyCode::Char('z'))), 1)]);
        }

        let bytes = [b"a\x1b", trigger, b"z", OSC, DA, trigger].concat();
        for split in 0..=bytes.len() {
            assert_eq!(
                parse_chunks(&[&bytes[..split], &bytes[split..]], true),
                vec![
                    (key(KeyCode::Char('a')), b"a".to_vec()),
                    (key(KeyCode::Escape), b"\x1b".to_vec()),
                    (key(KeyCode::Char('z')), b"z".to_vec()),
                    (osc(), OSC.to_vec()),
                    (da(), DA.to_vec()),
                ],
                "{trigger:?} split at {split}",
            );
        }

        let mut parser = InputParser::new();
        assert!(parser.parse_as_vec(trigger, true).is_empty());
    }
}

#[test]
fn fragmented_valid_sequences_keep_their_bytes() {
    let cases = [
        (
            b"\x1b[<0;12;34M".as_slice(),
            InputEvent::Mouse(MouseEvent {
                x: 12,
                y: 34,
                mouse_buttons: MouseButtons::LEFT,
                modifiers: Modifiers::NONE,
            }),
        ),
        (DA, da()),
        (OSC, osc()),
        (b"\x1b]11;rgb:1111/2222/3333\x07", osc()),
        (
            b"\x1b[?2026:1;2$y",
            InputEvent::DeviceControlReply {
                intermediates: b"$".to_vec(),
                params: b"?2026:1;2".to_vec(),
                final_byte: b'y',
                raw: b"\x1b[?2026:1;2$y".to_vec(),
            },
        ),
    ];
    for (bytes, expected) in cases {
        for split in 0..=bytes.len() {
            let mut parser = InputParser::new();
            let mut events = Vec::new();
            parser.parse_with_consumed(&bytes[..split], |event, n| events.push((event, n)), true);
            if split < bytes.len() {
                assert!(events.is_empty(), "premature events: {events:?}");
            }
            parser.parse_with_consumed(&bytes[split..], |event, n| events.push((event, n)), true);
            assert_eq!(events, vec![(Some(expected.clone()), bytes.len())]);
        }
    }
}

#[test]
fn recovery_preserves_parked_escape_and_paste() {
    for trigger in TRIGGERS {
        let bytes = [b"\x1b", *trigger, b"z", OSC, DA].concat();
        let events = parse_chunks(&[&bytes[..1], &bytes[1..]], true);
        assert_eq!(events[0], (key(KeyCode::Escape), b"\x1b".to_vec()));
        assert_eq!(events[events.len() - 2], (osc(), OSC.to_vec()));
        assert_eq!(events[events.len() - 1], (da(), DA.to_vec()));
    }

    let payload = [TRIGGERS.concat(), OSC.to_vec(), DA.to_vec()].concat();
    let paste = [b"\x1b[200~", payload.as_slice(), b"\x1b[201~"].concat();
    let bytes = [b"\x1b[?1\x01", paste.as_slice(), b"z", OSC, DA].concat();
    for split in 0..=bytes.len() {
        let events = parse_chunks(&[&bytes[..split], &bytes[split..]], true);
        assert_eq!(
            events[events.len() - 4],
            (
                InputEvent::Paste(String::from_utf8(payload.clone()).unwrap()),
                paste.clone(),
            )
        );
        assert_eq!(
            events[events.len() - 3],
            (key(KeyCode::Char('z')), b"z".to_vec())
        );
        assert_eq!(events[events.len() - 2], (osc(), OSC.to_vec()));
        assert_eq!(events[events.len() - 1], (da(), DA.to_vec()));
    }
}

#[test]
fn csi_scan_limit_recovers_without_eating_later_replies() {
    for prefix in [b"\x1b[?".as_slice(), b"\x1b[<", b"\x1b["] {
        let mut bytes = prefix.to_vec();
        bytes.resize(255, b'0');
        let mut parser = InputParser::new();
        assert!(parser.parse_as_vec(&bytes, true).is_empty());
        let events = parser.parse_as_vec(b"c", true);
        assert!(
            matches!(events.as_slice(), [InputEvent::DeviceControlReply { raw, .. }] if raw.len() == 256)
        );

        bytes.push(b'0');
        let mut parser = InputParser::new();
        let events = parser.parse_as_vec(&bytes, true);
        assert!(!events.is_empty(), "must recover at 256 bytes");
        assert!(events
            .iter()
            .all(|event| matches!(event, InputEvent::Key(_))));
        assert!(parser.buf.is_empty());
        assert_eq!(parser.parse_as_vec(OSC, true), vec![osc()]);
        assert_eq!(parser.parse_as_vec(DA, true), vec![da()]);

        let bytes = [bytes.as_slice(), b"cz", OSC, DA].concat();
        let events = parse_chunks(&[&bytes], true);
        assert_eq!(events[events.len() - 3].0, key(KeyCode::Char('z')));
        assert_eq!(events[events.len() - 2], (osc(), OSC.to_vec()));
        assert_eq!(events[events.len() - 1], (da(), DA.to_vec()));
    }
}

#[test]
fn no_more_still_falls_back_to_keys_for_unsupported_or_incomplete_input() {
    for trigger in
        TRIGGERS
            .iter()
            .copied()
            .chain([b"\x1b[?1".as_slice(), b"\x1b[<0;1", b"\x1b]11;incomplete"])
    {
        let events = parse_chunks(&[trigger], false);
        assert!(!events.is_empty());
        assert!(events
            .iter()
            .all(|(event, _)| matches!(event, InputEvent::Key(_))));
        assert_eq!(
            events
                .into_iter()
                .flat_map(|(_, raw)| raw)
                .collect::<Vec<_>>(),
            trigger
        );
    }
}
