use super::*;

#[test]
fn update_platforms_versions_and_cli() {
    assert_eq!(
        target("linux", "x86_64").unwrap(),
        "x86_64-unknown-linux-gnu"
    );
    assert!(target("linux", "aarch64").is_ok());
    assert!(target("macos", "aarch64").is_ok());
    assert!(target("macos", "x86_64").is_err());
    assert_eq!(version("v1.2.3").unwrap(), version("1.2.3").unwrap());
    assert!(version("1.2.3").unwrap() > version("1.2.3-dev").unwrap());
    assert!(version("bad").is_err());
    let release = Release {
        tag_name: "v1.2.3".into(),
        assets: vec![],
    };
    assert!(asset(&release, "missing").is_err());
}

#[test]
fn update_checksum_validation() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("archive");
    fs::write(&file, b"contents").unwrap();
    let digest = hex(&Sha256::digest(b"contents"));
    let sums = format!("{digest}  ./archive\n");
    verify(&file, &sums, "archive").unwrap();
    assert!(verify(&file, &(sums.clone() + &sums), "archive").is_err());
    assert!(verify(&file, "bad archive", "archive").is_err());
    assert!(verify(&file, &sums, "missing").is_err());
    fs::write(&file, b"changed").unwrap();
    assert!(verify(&file, &sums, "archive").is_err());
}

#[test]
fn update_install_replaces_target_and_preserves_unrelated_builtins() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    fs::create_dir_all(source.join("plugins/builtins")).unwrap();
    fs::write(source.join("yinz"), b"new").unwrap();
    fs::write(source.join("plugins/builtins/plugin.scm"), b"new plugin").unwrap();
    let data = temp.path().join("custom data");
    fs::create_dir_all(data.join("plugins/builtins")).unwrap();
    fs::write(data.join("plugins/builtins/plugin.scm"), b"old plugin").unwrap();
    fs::write(data.join("plugins/builtins/custom.scm"), b"custom").unwrap();
    let binary = temp.path().join("executable");
    fs::write(&binary, b"old").unwrap();
    let link = temp.path().join("link");
    std::os::unix::fs::symlink(&binary, &link).unwrap();
    install(&source, &link, &data).unwrap();
    assert!(link.is_symlink());
    assert_eq!(fs::read(&binary).unwrap(), b"new");
    assert_eq!(
        fs::read(data.join("plugins/builtins/plugin.scm")).unwrap(),
        b"new plugin"
    );
    assert_eq!(
        fs::read(data.join("plugins/builtins/custom.scm")).unwrap(),
        b"custom"
    );
    let bad_data = temp.path().join("bad data");
    std::os::unix::fs::symlink(&data, &bad_data).unwrap();
    fs::write(&binary, b"unchanged").unwrap();
    assert!(install(&source, &binary, &bad_data).is_err());
    assert_eq!(fs::read(&binary).unwrap(), b"unchanged");
}

#[test]
fn update_archive_rejects_links() {
    let temp = tempfile::tempdir().unwrap();
    let archive = temp.path().join("archive.tar.gz");
    let encoder = flate2::write::GzEncoder::new(
        fs::File::create(&archive).unwrap(),
        flate2::Compression::default(),
    );
    let mut builder = tar::Builder::new(encoder);
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Symlink);
    header.set_size(0);
    header.set_mode(0o777);
    builder
        .append_link(&mut header, "escape", "../outside")
        .unwrap();
    builder.into_inner().unwrap().finish().unwrap();
    assert!(extract(&archive, &temp.path().join("out")).is_err());
}

#[test]
fn update_fetch_failures_and_current_versions_do_not_install() {
    let temp = tempfile::tempdir().unwrap();
    let binary = temp.path().join("yinz");
    fs::write(&binary, b"old").unwrap();
    let error = run_with(None, &binary, "test", |_, _| Err(anyhow!("HTTP 403"))).unwrap_err();
    assert!(error.to_string().contains("403"));
    run_with(None, &binary, "test", |url, output| {
        assert_eq!(url, RELEASE_URL);
        output
            .write_all(br#"{"tag_name":"v0.0.0","assets":[]}"#)
            .unwrap();
        Ok(())
    })
    .unwrap();
    assert!(run_with(None, &binary, "test", |_, output| {
        output.write_all(b"invalid JSON").unwrap();
        Ok(())
    })
    .is_err());
    assert_eq!(fs::read(binary).unwrap(), b"old");
}

#[test]
fn update_fixture_release_installs_after_verification() {
    let temp = tempfile::tempdir().unwrap();
    let script = temp.path().join("fixture");
    yinz_test_support::write_warm_script(&script, "#!/bin/sh\necho 'yinz 999.0.0'\n");
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::default(),
    ));
    builder.append_path_with_name(&script, "yinz").unwrap();
    let builtins = temp.path().join("builtins");
    fs::create_dir(&builtins).unwrap();
    fs::write(builtins.join("test.scm"), b"test").unwrap();
    builder
        .append_dir_all("plugins/builtins", &builtins)
        .unwrap();
    let bytes = builder.into_inner().unwrap().finish().unwrap();
    let digest = hex(&Sha256::digest(&bytes));
    let binary = temp.path().join("installed");
    fs::write(&binary, b"old").unwrap();
    let data = temp.path().join("data");
    run_with(Some(&data), &binary, "test", |url, output| {
        let content = match url {
            RELEASE_URL => br#"{"tag_name":"v999.0.0","assets":[{"name":"yinz-test.tar.gz","browser_download_url":"archive"},{"name":"SHA256SUMS","browser_download_url":"sums"}]}"#.to_vec(),
            "archive" => bytes.clone(),
            "sums" => format!("{digest}  yinz-test.tar.gz\n").into_bytes(),
            _ => panic!("unexpected URL"),
        };
        output.write_all(&content).unwrap();
        Ok(())
    }).unwrap();
    assert_eq!(fs::read(binary).unwrap(), fs::read(script).unwrap());
    assert_eq!(
        fs::read(data.join("plugins/builtins/test.scm")).unwrap(),
        b"test"
    );
}
