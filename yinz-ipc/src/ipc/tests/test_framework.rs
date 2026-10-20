use std::collections::BTreeSet;
use std::sync::{Mutex, OnceLock};

/// The variant names the round-trip macros have actually been handed, so
/// `every_client_message_variant_is_covered` (and its server twin) can tell an untested variant
/// from a tested one. 374 hand-constructed values are a lot of effort to spend without knowing
/// which of the two enums' variants they reach.
fn covered(which: Which) -> &'static Mutex<BTreeSet<&'static str>> {
    static CLIENT: OnceLock<Mutex<BTreeSet<&'static str>>> = OnceLock::new();
    static SERVER: OnceLock<Mutex<BTreeSet<&'static str>>> = OnceLock::new();
    match which {
        Which::Client => CLIENT.get_or_init(Default::default),
        Which::Server => SERVER.get_or_init(Default::default),
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Which {
    Client,
    Server,
}

pub(crate) fn record_covered(which: Which, variant: &'static str) {
    if let Ok(mut covered) = covered(which).lock() {
        covered.insert(variant);
    }
}

/// The variants of `enum_variants` that no round-trip invocation ever built.
pub(crate) fn uncovered_variants(
    which: Which,
    enum_variants: &[&'static str],
) -> Vec<&'static str> {
    let Ok(covered) = covered(which).lock() else {
        return vec![];
    };
    enum_variants
        .iter()
        .copied()
        .filter(|variant| !covered.contains(variant))
        .collect()
}

/// Exercise the encoded wire too: struct conversions cannot catch missing oneof tags.
macro_rules! test_client_roundtrip {
    ($msg:expr_2021) => {{
        let original: crate::ipc::ClientToServerMsg = $msg;
        crate::ipc::tests::test_framework::record_covered(
            crate::ipc::tests::test_framework::Which::Client,
            <&'static str as From<&crate::ipc::ClientToServerMsg>>::from(&original),
        );
        let proto: crate::client_server_contract::client_server_contract::ClientToServerMsg =
            original
                .clone()
                .try_into()
                .expect("Failed to convert to protobuf");
        let bytes = ::prost::Message::encode_to_vec(&proto);
        let proto: crate::client_server_contract::client_server_contract::ClientToServerMsg =
            ::prost::Message::decode(bytes.as_slice()).expect("Failed to decode protobuf bytes");
        let roundtrip: crate::ipc::ClientToServerMsg = proto
            .try_into()
            .expect("Failed to convert back from protobuf");
        assert_eq!(original, roundtrip);
    }};
}

/// Exercise the encoded wire for every ServerToClientMsg fixture as well.
macro_rules! test_server_roundtrip {
    ($msg:expr_2021) => {{
        let original: crate::ipc::ServerToClientMsg = $msg;
        crate::ipc::tests::test_framework::record_covered(
            crate::ipc::tests::test_framework::Which::Server,
            <&'static str as From<&crate::ipc::ServerToClientMsg>>::from(&original),
        );
        let proto: crate::client_server_contract::client_server_contract::ServerToClientMsg =
            original.clone().into();
        let bytes = ::prost::Message::encode_to_vec(&proto);
        let proto: crate::client_server_contract::client_server_contract::ServerToClientMsg =
            ::prost::Message::decode(bytes.as_slice()).expect("Failed to decode protobuf bytes");
        let roundtrip: crate::ipc::ServerToClientMsg = proto
            .try_into()
            .expect("Failed to convert back from protobuf");
        assert_eq!(original, roundtrip);
    }};
}

pub(crate) use {test_client_roundtrip, test_server_roundtrip};
