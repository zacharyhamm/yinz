use super::*;
use crate::client_server_contract::client_server_contract as wire;
use crate::ipc::ClientToServerMsg;
use prost::Message;
use yinz_types::keys::KeyWithModifier;

#[test]
fn extended_function_keys_round_trip_through_client_messages() {
    for number in 1..=35 {
        let key = KeyWithModifier::new(BareKey::F(number))
            .with_ctrl_modifier()
            .with_alt_modifier()
            .with_shift_modifier()
            .with_super_modifier();
        let message = ClientToServerMsg::Key {
            key,
            raw_bytes: format!("\x1b[{};16u", 57363 + u32::from(number)).into_bytes(),
            is_kitty_keyboard_protocol: true,
        };
        let encoded = wire::ClientToServerMsg::try_from(message.clone())
            .unwrap()
            .encode_to_vec();
        let decoded = wire::ClientToServerMsg::decode(encoded.as_slice()).unwrap();
        assert_eq!(ClientToServerMsg::try_from(decoded).unwrap(), message);
    }
    for (key, bytes) in [
        (BareKey::F(1), vec![8, 12]),
        (BareKey::F(12), vec![8, 23]),
        (BareKey::Enter, vec![8, 27]),
    ] {
        assert_eq!(
            wire::KeyWithModifier::from(KeyWithModifier::new(key)).encode_to_vec(),
            bytes,
        );
    }
    for invalid in [-1, 0, 57, i32::MAX] {
        assert!(bare_key_from_proto_i32(invalid).is_err());
    }
}
