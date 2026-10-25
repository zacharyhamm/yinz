use super::*;
use prost::Message;

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
