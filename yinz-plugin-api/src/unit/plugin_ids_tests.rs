use super::*;
use prost::Message;

const _: fn(
    yinz_types::multiplayer::ColorSlot,
    yinz_types::data::MultiplayerColors,
) -> (
    yinz_types::data::PaletteColor,
    yinz_types::data::PaletteColor,
) = yinz_types::multiplayer::ColorSlot::colors;
const _: fn(
    yinz_types::internal_ids::PluginId,
    ClientId,
    std::collections::BTreeMap<String, String>,
) -> yinz_types::data::OriginatingPlugin = yinz_types::data::OriginatingPlugin::new;

#[test]
fn installation_identity_preserves_fixed_wire_bytes() {
    let bytes = b"\x32\x06host-a";
    let wire = ProtobufPluginIds::decode(bytes.as_slice()).unwrap();
    let ids = PluginIds::try_from(wire).unwrap();
    assert_eq!(ids.host_id, HostId::new("host-a"));
    assert_eq!(serde_json::to_value(&ids).unwrap()["host_id"], "host-a");
    assert_eq!(
        ProtobufPluginIds::try_from(ids).unwrap().encode_to_vec(),
        bytes
    );
    assert!(PluginIds::try_from(ProtobufPluginIds::default())
        .unwrap()
        .host_id
        .is_empty());
}

#[test]
fn process_identity_preserves_fixed_legacy_wire_bits() {
    use crate::plugin_command::{get_pane_pid_response, ProtobufGetPanePidResponse};
    use yinz_types::{data::GetPanePidResponse, internal_ids::ProcessId};
    let bytes = [
        0x10, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01,
    ];
    let wire = ProtobufPluginIds::decode(bytes.as_slice()).unwrap();
    let ids = PluginIds::try_from(wire).unwrap();
    assert_eq!(ids.yinz_pid, ProcessId::new(u32::MAX));
    assert_eq!(serde_json::to_value(&ids).unwrap()["yinz_pid"], u32::MAX);
    assert_eq!(
        ProtobufPluginIds::try_from(ids).unwrap().encode_to_vec(),
        bytes
    );
    for signed in [i32::MIN, -1, 0, i32::MAX] {
        let wire = ProtobufGetPanePidResponse {
            result: Some(get_pane_pid_response::Result::Pid(signed)),
        };
        let response = GetPanePidResponse::try_from(wire.clone()).unwrap();
        assert_eq!(
            response,
            GetPanePidResponse::Ok(ProcessId::from_legacy_signed_bits(signed))
        );
        assert_eq!(ProtobufGetPanePidResponse::from(response), wire);
    }
}

#[test]
fn plugin_identity_preserves_all_legacy_wire_bits() {
    // This protobuf field is int32, but legacy senders use all 32 bits for an unsigned ID.
    let bytes = [
        0x08, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01,
    ];
    let wire = ProtobufPluginIds::decode(bytes.as_slice()).unwrap();
    let ids = PluginIds::try_from(wire).unwrap();
    assert_eq!(
        ids.plugin_id,
        yinz_types::internal_ids::PluginId::new(u32::MAX)
    );
    assert_eq!(serde_json::to_value(&ids).unwrap()["plugin_id"], u32::MAX);
    assert_eq!(
        ProtobufPluginIds::try_from(ids).unwrap().encode_to_vec(),
        bytes
    );
}

#[test]
fn client_identity_preserves_fixed_protobuf_bytes_and_json() {
    for (client_id, bytes) in [
        (0, vec![]),
        (u32::MAX, vec![0x20, 0xff, 0xff, 0xff, 0xff, 0x0f]),
    ] {
        let wire = ProtobufPluginIds::decode(bytes.as_slice()).unwrap();
        let ids = PluginIds::try_from(wire).unwrap();
        assert_eq!(serde_json::to_value(&ids).unwrap()["client_id"], client_id);
        let wire = ProtobufPluginIds::try_from(ids).unwrap();
        assert_eq!(wire.encode_to_vec(), bytes);
    }
}
