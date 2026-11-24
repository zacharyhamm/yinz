use super::*;
use yinz_types::data::{ClientInfo, PaneGitProbe};

#[test]
fn remote_git_probe_keeps_large_metadata_and_config_compact() {
    let probe = PaneGitProbe {
        host: "remote".into(),
        cwd: "/project".into(),
        exit_code: Some(128),
        metadata: vec![255; 200_001],
        config: vec![0; 200_001],
    };
    let value = git_probe(probe.clone()).unwrap().expect_json();
    assert_eq!(value["host"], "remote");
    assert_eq!(value["cwd"], "/project");
    assert_eq!(value["exit_code"], 128);
    for (field, bytes) in [("metadata", probe.metadata), ("config", probe.config)] {
        assert!(
            value[field].is_object(),
            "probe {field} expanded into a JSON array"
        );
        assert_eq!(yinz_steel::binary::decode(&value[field]).unwrap(), bytes);
    }
}

#[test]
fn older_client_info_defaults_to_an_unavailable_timezone() {
    let info: ClientInfo = serde_json::from_value(serde_json::json!({
        "client_id":7,"pane_id":{"Terminal":1},"running_command":"shell","is_current_client":true
    }))
    .unwrap();
    assert_eq!(info.timezone, None);
}

#[test]
fn plugin_ids_preserve_maximum_unsigned_json_scalars() {
    let fields = serde_json::json!({
        "plugin_id": u32::MAX, "client_id": u32::MAX, "yinz_pid": u32::MAX,
        "initial_cwd": "/work", "hostname": "", "host_id": "", "max_session_name_bytes": null
    });
    let ids: yinz_types::data::PluginIds = serde_json::from_value(fields.clone()).unwrap();
    assert_eq!(ids.plugin_id.get(), u32::MAX);
    assert_eq!(ids.client_id.get(), u32::MAX);
    assert_eq!(ids.yinz_pid.get(), u32::MAX);
    assert_eq!(value(ids).unwrap().expect_json(), fields);
}
