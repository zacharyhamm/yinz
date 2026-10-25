use super::*;

#[test]
fn shared_decoder_distinguishes_missing_unknown_invalid_and_failed_replies() {
    assert_eq!(
        decode_response(CommandType::GetPaneCwd, &[])
            .unwrap()
            .unwrap(),
        DecodedResponse::Json(serde_json::Value::Null)
    );
    assert!(decode_response(CommandType::Subscribe, &[]).is_none());
    assert!(decode_response(CommandType::GetPaneCwd, &[255])
        .unwrap()
        .unwrap_err()
        .to_string()
        .contains("could not be read"));
    let failed = ProtobufFsReadResponse {
        result: Some(fs_read_response::Result::Error("denied".into())),
    }
    .encode_to_vec();
    assert_eq!(
        decode_response(CommandType::FsRead, &failed)
            .unwrap()
            .unwrap_err()
            .to_string(),
        "denied"
    );
}

#[test]
fn remote_git_probe_keeps_large_metadata_and_config_compact() {
    let probe = PaneGitProbe {
        host: "remote".into(),
        cwd: "/project".into(),
        exit_code: Some(128),
        metadata: vec![255; 200_001],
        config: vec![0; 200_001],
    };
    let response = ProtobufGetPaneGitProbeResponse::from(
        yinz_types::data::GetPaneGitProbeResponse::Ok(probe.clone()),
    )
    .encode_to_vec();
    let DecodedResponse::Json(value) = decode_response(CommandType::GetPaneGitProbe, &response)
        .unwrap()
        .unwrap()
    else {
        panic!("expected probe object")
    };
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
