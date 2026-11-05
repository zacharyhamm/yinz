use super::*;

#[test]
fn discovery_recovery_keeps_pinned_prerelease_and_original_channel() {
    let root = tempfile::tempdir().unwrap();
    let socket = root.path().join("session");
    let data = root.path().join("data");
    let operation = uuid::Uuid::new_v4().to_string();
    let mut store = Store::open_with_channel(
        &socket,
        &data,
        &operation,
        false,
        ReleaseChannel::Prerelease,
    )
    .unwrap();
    let release = resolve_release(&mut store, &mut |url, out| {
        let content = if url.ends_with("page=1") {
            br#"[{"tag_name":"v2.0.0-alpha.1","assets":[]}]"#.as_slice()
        } else {
            b"[]"
        };
        out.write_all(content).unwrap();
        Ok(())
    })
    .unwrap();
    let pinned = serde_json::to_value(release).unwrap();
    drop(store);
    // The same channel resumes the original operation without rediscovery.
    let mut resumed = Store::open_with_channel(
        &socket,
        &data,
        &uuid::Uuid::new_v4().to_string(),
        false,
        ReleaseChannel::Prerelease,
    )
    .unwrap();
    assert_eq!(resumed.journal.operation, operation);
    assert_eq!(resumed.journal.channel, ReleaseChannel::Prerelease);
    assert!(!resumed.journal.source_build);
    let release = resolve_release(&mut resumed, &mut |_, _| {
        panic!("must not rediscover pinned release")
    })
    .unwrap();
    assert_eq!(serde_json::to_value(release).unwrap(), pinned);
}

#[test]
fn prerelease_participants_require_capability_but_stable_and_source_do_not() {
    let mut installation = Installation {
        host: "host".into(),
        executable: "/installed/yinz".into(),
        data_dir: "/data".into(),
        running_version: "1.0.0".into(),
        installed_version: "1.0.0".into(),
        target: "test".into(),
        capabilities: capabilities(),
    };
    let mut release = update::Release {
        channel: ReleaseChannel::Prerelease,
        tag_name: "2.0.0-alpha.1".into(),
        assets: vec![],
        source: None,
    };
    require_prerelease_support(&release, &installation).unwrap();
    let mut old = serde_json::to_value(&installation.capabilities).unwrap();
    old.as_object_mut().unwrap().remove("prerelease_update");
    installation.capabilities = serde_json::from_value(old).unwrap();
    let error = require_prerelease_support(&release, &installation).unwrap_err();
    assert!(error.to_string().contains("update it manually first"));
    release.source = Some(update::SourceRelease {
        commit: "source".into(),
        artifacts: vec![],
    });
    require_prerelease_support(&release, &installation).unwrap();
    release.source = None;
    release.tag_name = "2.0.0".into();
    require_prerelease_support(&release, &installation).unwrap();
}
