pub use super::generated_api::api::plugin_ids::{
    PluginIds as ProtobufPluginIds, YinzVersion as ProtobufYinzVersion,
};
use yinz_types::internal_ids::HostId;
use yinz_types::internal_ids::ProcessId;
use {yinz_types::data::ClientId, yinz_types::data::PluginIds};

use std::convert::TryFrom;
use std::path::PathBuf;

impl TryFrom<ProtobufPluginIds> for PluginIds {
    type Error = &'static str;
    fn try_from(protobuf_plugin_ids: ProtobufPluginIds) -> Result<Self, &'static str> {
        Ok(PluginIds {
            plugin_id: yinz_types::internal_ids::PluginId::new(
                protobuf_plugin_ids.plugin_id as u32,
            ),
            yinz_pid: ProcessId::from_legacy_signed_bits(protobuf_plugin_ids.yinz_pid),
            initial_cwd: PathBuf::from(protobuf_plugin_ids.initial_cwd),
            client_id: ClientId::new(protobuf_plugin_ids.client_id),
            hostname: protobuf_plugin_ids.hostname,
            host_id: HostId::new(protobuf_plugin_ids.host_id),
            max_session_name_bytes: protobuf_plugin_ids
                .max_session_name_bytes
                .map(|n| n as usize),
        })
    }
}

impl TryFrom<PluginIds> for ProtobufPluginIds {
    type Error = &'static str;
    fn try_from(plugin_ids: PluginIds) -> Result<Self, &'static str> {
        Ok(ProtobufPluginIds {
            plugin_id: plugin_ids.plugin_id.get() as i32,
            yinz_pid: plugin_ids.yinz_pid.legacy_signed_bits(),
            initial_cwd: plugin_ids.initial_cwd.display().to_string(),
            client_id: plugin_ids.client_id.get(),
            hostname: plugin_ids.hostname,
            host_id: plugin_ids.host_id.into_string(),
            max_session_name_bytes: plugin_ids
                .max_session_name_bytes
                .map(u32::try_from)
                .transpose()
                .map_err(|_| "max_session_name_bytes is outside u32 range")?,
        })
    }
}

impl TryFrom<&str> for ProtobufYinzVersion {
    type Error = &'static str;
    fn try_from(yinz_version: &str) -> Result<Self, &'static str> {
        Ok(ProtobufYinzVersion {
            version: yinz_version.to_owned(),
        })
    }
}

#[cfg(test)]
#[path = "unit/plugin_ids_tests.rs"]
mod tests;
