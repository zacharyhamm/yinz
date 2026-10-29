use super::*;
use prost::Message;

#[test]
fn extended_function_keys_round_trip_through_plugin_events_and_actions() {
    use crate::action::ProtobufKeyWithModifier;
    for number in 1..=35 {
        let key = KeyWithModifier::new(BareKey::F(number))
            .with_ctrl_modifier()
            .with_alt_modifier()
            .with_shift_modifier()
            .with_super_modifier();
        let event_key = ProtobufKey::try_from(key.clone()).unwrap();
        let decoded = ProtobufKey::decode(event_key.encode_to_vec().as_slice()).unwrap();
        assert_eq!(KeyWithModifier::try_from(decoded).unwrap(), key);
        let action_key = ProtobufKeyWithModifier::try_from(key.clone()).unwrap();
        let decoded =
            ProtobufKeyWithModifier::decode(action_key.encode_to_vec().as_slice()).unwrap();
        assert_eq!(KeyWithModifier::try_from(decoded).unwrap(), key);
    }
    for (key, bytes) in [
        (BareKey::F(1), vec![16, 11]),
        (BareKey::F(12), vec![16, 22]),
        (BareKey::Enter, vec![16, 31]),
    ] {
        assert_eq!(
            ProtobufKey::try_from(KeyWithModifier::new(key))
                .unwrap()
                .encode_to_vec(),
            bytes,
        );
    }
    for number in [0, 36, u8::MAX] {
        let key = KeyWithModifier::new(BareKey::F(number));
        assert!(ProtobufKey::try_from(key.clone()).is_err());
        assert!(ProtobufKeyWithModifier::try_from(key).is_err());
    }
}

#[test]
fn keys_preserve_protobuf_bytes_and_reject_invalid_scalars() {
    for (key, bytes) in [
        (KeyWithModifier::new(BareKey::Char('\0')), vec![24, 0]),
        (
            KeyWithModifier::new(BareKey::Char('\u{10ffff}')).with_ctrl_modifier(),
            vec![8, 0, 24, 255, 255, 67],
        ),
    ] {
        assert_eq!(
            ProtobufKey::try_from(key.clone()).unwrap().encode_to_vec(),
            bytes
        );
        assert_eq!(
            KeyWithModifier::try_from(ProtobufKey::decode(bytes.as_slice()).unwrap()).unwrap(),
            key
        );
    }
    for invalid in [-1, 0xd800, 0x110000, i32::MAX] {
        assert!(BareKey::try_from(ProtobufMainKey::Char(invalid)).is_err());
    }
}
