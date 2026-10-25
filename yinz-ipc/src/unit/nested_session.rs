use super::*;
use yinz_types::data::BareKey;

#[test]
fn session_identity_keeps_fixed_protobuf_bytes() {
    let cases = [
        (
            NestedSessionMessage::Announce {
                session_name: SessionName::new("main"),
                capabilities: vec![],
            },
            vec![10, 6, 10, 4, b'm', b'a', b'i', b'n'],
        ),
        (
            NestedSessionMessage::AncestryUpdate {
                ancestry: vec![SessionName::new("a"), SessionName::new("b")],
            },
            vec![98, 6, 10, 1, b'a', 10, 1, b'b'],
        ),
    ];
    for (message, bytes) in cases {
        let wire = proto::NestedSessionMessage::from(message.clone());
        assert_eq!(wire.encode_to_vec(), bytes);
        let decoded = proto::NestedSessionMessage::decode(bytes.as_slice()).unwrap();
        assert_eq!(NestedSessionMessage::try_from(decoded).unwrap(), message);
    }
}

fn all_message_arms() -> Vec<NestedSessionMessage> {
    vec![
        NestedSessionMessage::Announce {
            session_name: "guest".to_owned().into(),
            capabilities: vec![NestedSessionCapability::NestedControl],
        },
        NestedSessionMessage::FocusHost {
            direction: Some(Direction::Left),
        },
        NestedSessionMessage::FocusHost { direction: None },
        NestedSessionMessage::ToggleHostFullscreen { fullscreen: true },
        NestedSessionMessage::Pong,
        NestedSessionMessage::Bye,
        NestedSessionMessage::AnnounceAck {
            ancestry: vec!["outer".to_owned().into(), "middle".to_owned().into()],
            capabilities: vec![NestedSessionCapability::NestedControl],
            descend_keys: vec![
                KeyWithModifier::new(BareKey::Char('o')).with_ctrl_modifier(),
                KeyWithModifier::new(BareKey::Down),
            ],
        },
        NestedSessionMessage::FocusGained {
            from_direction: Some(Direction::Right),
        },
        NestedSessionMessage::FocusLost,
        NestedSessionMessage::FullscreenState { fullscreen: true },
        NestedSessionMessage::AncestryUpdate {
            ancestry: vec!["outer".to_owned().into()],
        },
        NestedSessionMessage::Ping,
        NestedSessionMessage::ShortcutUpdate {
            ascend_keys: vec![
                KeyWithModifier::new(BareKey::Char('o')).with_ctrl_modifier(),
                KeyWithModifier::new(BareKey::Up),
            ],
            descend_keys: vec![],
        },
    ]
}

#[test]
fn payload_roundtrip_preserves_every_arm() {
    for message in all_message_arms() {
        let decoded = decode_payload(&encode_payload(&message));
        assert_eq!(decoded, Some(message));
    }
}

#[test]
fn focus_direction_roundtrip_preserves_every_direction() {
    let directions = [
        None,
        Some(Direction::Left),
        Some(Direction::Right),
        Some(Direction::Up),
        Some(Direction::Down),
    ];
    for direction in directions {
        let focus_host = NestedSessionMessage::FocusHost { direction };
        assert_eq!(
            decode_payload(&encode_payload(&focus_host)),
            Some(focus_host)
        );
        let focus_gained = NestedSessionMessage::FocusGained {
            from_direction: direction,
        };
        assert_eq!(
            decode_payload(&encode_payload(&focus_gained)),
            Some(focus_gained)
        );
    }
}

#[test]
fn frame_roundtrip_preserves_message() {
    let message = NestedSessionMessage::Announce {
        session_name: "guest".to_owned().into(),
        capabilities: vec![NestedSessionCapability::NestedControl],
    };
    let frame = encode_frame(&message);
    assert!(frame.starts_with(NESTED_FRAME_HEADER));
    assert!(frame.ends_with(NESTED_FRAME_TERMINATOR));
    let encoded_payload =
        &frame[NESTED_FRAME_HEADER.len()..frame.len() - NESTED_FRAME_TERMINATOR.len()];
    let payload = decode_base64(encoded_payload).unwrap();
    assert_eq!(decode_payload(&payload), Some(message));
}

#[test]
fn garbage_base64_is_rejected() {
    assert_eq!(decode_base64(b"!!!not-base64!!!"), None);
}

#[test]
fn truncated_payload_is_rejected() {
    let payload = encode_payload(&NestedSessionMessage::Announce {
        session_name: "a-long-session-name-to-truncate".to_owned().into(),
        capabilities: vec![],
    });
    assert_eq!(decode_payload(&payload[..payload.len() - 5]), None);
}

#[test]
fn unknown_oneof_arm_is_rejected() {
    let unknown_field_bytes = [0xE2, 0x03, 0x00];
    assert_eq!(decode_payload(&unknown_field_bytes), None);
}

#[test]
fn empty_payload_is_rejected() {
    assert_eq!(decode_payload(&[]), None);
}

fn test_scheduler(now: Instant) -> ReannounceScheduler {
    ReannounceScheduler::with_settings(now, Duration::from_millis(3000), 5)
}

#[test]
fn unacked_announces_stop_at_the_budget() {
    let start = Instant::now();
    let mut scheduler = test_scheduler(start);
    let mut announces = 1;
    for tick in 1..=30 {
        if scheduler.on_tick(start + Duration::from_millis(tick * 1000)) {
            announces += 1;
        }
    }
    assert_eq!(announces, 5);
    assert!(scheduler.budget_exhausted());
    assert!(!scheduler.host_contacted());
}

#[test]
fn silence_threshold_is_respected_before_announcing() {
    let start = Instant::now();
    let mut scheduler = test_scheduler(start);
    assert!(!scheduler.on_tick(start + Duration::from_millis(1000)));
    assert!(!scheduler.on_tick(start + Duration::from_millis(2999)));
    assert!(scheduler.on_tick(start + Duration::from_millis(3000)));
}

#[test]
fn host_contact_lifts_the_budget_and_resets_the_silence_window() {
    let start = Instant::now();
    let mut scheduler = test_scheduler(start);
    for tick in 1..=30 {
        scheduler.on_tick(start + Duration::from_millis(tick * 1000));
    }
    assert!(scheduler.budget_exhausted());
    assert!(scheduler.note_host_contact(start + Duration::from_millis(31_000)));
    assert!(scheduler.host_contacted());
    assert!(!scheduler.budget_exhausted());
    assert!(!scheduler.on_tick(start + Duration::from_millis(32_000)));
    for tick in 34..=100 {
        assert!(scheduler.on_tick(start + Duration::from_millis(tick * 1000)));
    }
}

#[test]
fn repeated_host_contact_is_only_first_contact_once() {
    let start = Instant::now();
    let mut scheduler = test_scheduler(start);
    assert!(scheduler.note_host_contact(start));
    assert!(!scheduler.note_host_contact(start + Duration::from_millis(1000)));
}
