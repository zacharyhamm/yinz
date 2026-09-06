pub use super::generated_api::api::plugin_ids::{
    PluginIds as ProtobufPluginIds, ZellijVersion as ProtobufZellijVersion,
};
use crate::data::{ClientId, PluginIds};

use std::convert::TryFrom;
use std::path::PathBuf;

impl TryFrom<ProtobufPluginIds> for PluginIds {
    type Error = &'static str;
    fn try_from(protobuf_plugin_ids: ProtobufPluginIds) -> Result<Self, &'static str> {
        Ok(PluginIds {
            plugin_id: protobuf_plugin_ids.plugin_id as u32,
            zellij_pid: protobuf_plugin_ids.zellij_pid as u32,
            initial_cwd: PathBuf::from(protobuf_plugin_ids.initial_cwd),
            client_id: ClientId::try_from(protobuf_plugin_ids.client_id)
                .map_err(|_| "client_id is outside u16 range")?,
            hostname: protobuf_plugin_ids.hostname,
            host_id: protobuf_plugin_ids.host_id,
        })
    }
}

impl TryFrom<PluginIds> for ProtobufPluginIds {
    type Error = &'static str;
    fn try_from(plugin_ids: PluginIds) -> Result<Self, &'static str> {
        Ok(ProtobufPluginIds {
            plugin_id: plugin_ids.plugin_id as i32,
            zellij_pid: plugin_ids.zellij_pid as i32,
            initial_cwd: plugin_ids.initial_cwd.display().to_string(),
            client_id: u32::from(plugin_ids.client_id.get()),
            hostname: plugin_ids.hostname,
            host_id: plugin_ids.host_id,
        })
    }
}

impl TryFrom<&str> for ProtobufZellijVersion {
    type Error = &'static str;
    fn try_from(zellij_version: &str) -> Result<Self, &'static str> {
        Ok(ProtobufZellijVersion {
            version: zellij_version.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    const _: fn(
        ClientId,
        crate::data::MultiplayerColors,
    ) -> Option<(crate::data::PaletteColor, crate::data::PaletteColor)> =
        crate::data::client_id_to_colors;
    const _: fn(
        u32,
        ClientId,
        std::collections::BTreeMap<String, String>,
    ) -> crate::data::OriginatingPlugin = crate::data::OriginatingPlugin::new;

    #[test]
    fn client_identity_preserves_fixed_protobuf_bytes_and_json() {
        for (client_id, bytes) in [(0, vec![]), (u16::MAX, vec![0x20, 0xff, 0xff, 0x03])] {
            let wire = ProtobufPluginIds::decode(bytes.as_slice()).unwrap();
            let ids = PluginIds::try_from(wire).unwrap();
            assert_eq!(serde_json::to_value(&ids).unwrap()["client_id"], client_id);
            let wire = ProtobufPluginIds::try_from(ids).unwrap();
            assert_eq!(wire.encode_to_vec(), bytes);
        }
    }

    #[test]
    fn out_of_range_client_identity_is_rejected() {
        for client_id in [65536, u32::MAX] {
            assert!(PluginIds::try_from(ProtobufPluginIds {
                client_id,
                ..Default::default()
            })
            .is_err());
        }
    }
}
