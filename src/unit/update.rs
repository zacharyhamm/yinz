use super::*;
use std::path::PathBuf;

#[test]
fn update_cli_recovery_requires_all_explicit_targets() {
    // Match the existing CLI parser tests: the debug Clap builder needs more
    // than libtest's default thread stack. Production CLI parsing runs on main.
    std::thread::Builder::new().stack_size(32 * 1024 * 1024).spawn(|| {
    use clap::Parser;
    use yinz_cli::args::{CliArgs, Command};
    let ordinary = CliArgs::try_parse_from(["yinz", "update"]).unwrap();
    assert!(matches!(
        ordinary.command,
        Some(Command::Update {
            resume_client: None,
            client: None,
            prerelease: false,
            session: None
        })
    ));
    let opted_in = CliArgs::try_parse_from(["yinz", "update", "--prerelease"]).unwrap();
    assert!(matches!(opted_in.command, Some(Command::Update { prerelease: true, .. })));
    assert!(CliArgs::try_parse_from(["yinz", "update", "--prerelease", "--resume-client", "3", "--client", "9", "--session", "work"]).is_err());
    let recovery = CliArgs::try_parse_from([
        "yinz",
        "--data-dir",
        "/custom/data",
        "update",
        "--resume-client",
        "3",
        "--client",
        "9",
        "--session",
        "work",
    ])
    .unwrap();
    assert!(
        matches!(recovery.command, Some(Command::Update { resume_client: Some(3), client: Some(9), session: Some(ref name), prerelease: false }) if name == "work")
    );
    for arguments in [
        vec!["yinz", "update", "--resume-client", "3"],
        vec!["yinz", "update", "--client", "9", "--session", "work"],
        vec![
            "yinz",
            "update",
            "--resume-client",
            "0",
            "--client",
            "9",
            "--session",
            "work",
        ],
    ] {
        assert!(CliArgs::try_parse_from(arguments).is_err());
    }
    }).unwrap().join().unwrap();
}

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
        channel: Default::default(),
        source: None,
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
    yinz_test_support::write_warm_script(&binary, "#!/bin/sh\necho 'yinz 0.0.1'\n");
    let original = fs::read(&binary).unwrap();
    let data = temp.path().join("data");
    let error = run_with(
        Some(&data),
        &binary,
        "test",
        ReleaseChannel::Stable,
        |_, _| Err(anyhow!("HTTP 403")),
    )
    .unwrap_err();
    assert!(error.to_string().contains("403"));
    run_with(
        Some(&data),
        &binary,
        "test",
        ReleaseChannel::Stable,
        |url, output| {
            assert_eq!(url, RELEASE_URL);
            output
                .write_all(br#"{"tag_name":"v0.0.0","assets":[]}"#)
                .unwrap();
            Ok(())
        },
    )
    .unwrap();
    assert!(run_with(
        Some(&data),
        &binary,
        "test",
        ReleaseChannel::Stable,
        |_, output| {
            output.write_all(b"invalid JSON").unwrap();
            Ok(())
        }
    )
    .is_err());
    assert_eq!(fs::read(binary).unwrap(), original);
}

#[test]
fn update_fixture_release_installs_after_verification() {
    for channel in [ReleaseChannel::Stable, ReleaseChannel::Prerelease] {
        let selected = if channel == ReleaseChannel::Stable {
            "999.0.0"
        } else {
            "999.0.0-alpha.1"
        };
        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("fixture");
        yinz_test_support::write_warm_script(
            &script,
            &format!("#!/bin/sh\necho 'yinz {selected}'\n"),
        );
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
        yinz_test_support::write_warm_script(&binary, "#!/bin/sh\necho 'yinz 0.0.1'\n");
        let data = temp.path().join("data");
        run_with(Some(&data), &binary, "test", channel, |url, output| {
        let metadata = serde_json::json!({"tag_name":format!("v{selected}"),"assets":[{"name":"yinz-test.tar.gz","browser_download_url":"archive"},{"name":"SHA256SUMS","browser_download_url":"sums"}]});
        let content = match url {
            RELEASE_URL => serde_json::to_vec(&metadata).unwrap(),
            url if url.ends_with("page=1") => serde_json::to_vec(&vec![metadata]).unwrap(),
            url if url.ends_with("page=2") => b"[]".to_vec(),
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
}

#[test]
fn preparation_is_pinned_and_does_not_install() {
    for selected in ["999.0.0", "999.0.0-alpha.1"] {
        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("fixture");
        yinz_test_support::write_warm_script(
            &script,
            &format!("#!/bin/sh\necho 'yinz {selected}'\n"),
        );
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::default(),
        ));
        builder.append_path_with_name(&script, "yinz").unwrap();
        let builtins = temp.path().join("builtins");
        fs::create_dir(&builtins).unwrap();
        fs::write(builtins.join("test.scm"), b"new plugin").unwrap();
        builder
            .append_dir_all("plugins/builtins", &builtins)
            .unwrap();
        let bytes = builder.into_inner().unwrap().finish().unwrap();
        let digest = hex(&Sha256::digest(&bytes));
        let release = Release {
            channel: ReleaseChannel::Prerelease,
            source: None,
            tag_name: format!("v{selected}"),
            assets: vec![
                Asset {
                    name: "yinz-test.tar.gz".into(),
                    browser_download_url: "archive".into(),
                },
                Asset {
                    name: "SHA256SUMS".into(),
                    browser_download_url: "sums".into(),
                },
            ],
        };
        let binary = temp.path().join("installed");
        fs::write(&binary, b"old binary").unwrap();
        let data = temp.path().join("installed-data");
        fs::create_dir_all(data.join("plugins/builtins")).unwrap();
        fs::write(data.join("plugins/builtins/test.scm"), b"old plugin").unwrap();
        let mut phases = Vec::new();
        let mut download = |url: &str, output: &mut dyn std::io::Write| {
            match url {
                "archive" => output.write_all(&bytes).unwrap(),
                "sums" => output
                    .write_all(format!("{digest}  yinz-test.tar.gz\n").as_bytes())
                    .unwrap(),
                _ => panic!("preparation must not resolve latest again: {url}"),
            }
            Ok(())
        };
        let prepared = prepare(&release, "test", &mut download, &mut |phase| {
            phases.push(phase)
        })
        .unwrap();
        assert_eq!(
            phases,
            vec![
                Progress::Downloading,
                Progress::Verifying,
                Progress::Validating
            ]
        );
        assert_eq!(prepared.version, version(selected).unwrap());
        assert_eq!(fs::read(&binary).unwrap(), b"old binary");
        assert_eq!(
            fs::read(data.join("plugins/builtins/test.scm")).unwrap(),
            b"old plugin"
        );
        // SSH bootstrap uses this same path for foreign artifacts. It must allow
        // the pinned prerelease without executing the foreign binary on this host.
        phases.clear();
        let foreign = prepare_artifact(&release, "test", &mut download, &mut |phase| {
            phases.push(phase)
        })
        .unwrap();
        assert_eq!(foreign.version, version(selected).unwrap());
        assert_eq!(phases, vec![Progress::Downloading, Progress::Verifying]);
        let mismatch = prepare(
            &release,
            "test",
            &mut |url, output| {
                if url == "sums" {
                    output
                        .write_all(format!("{}  yinz-test.tar.gz\n", "0".repeat(64)).as_bytes())
                        .unwrap();
                    Ok(())
                } else {
                    download(url, output)
                }
            },
            &mut |_| {},
        )
        .err()
        .unwrap();
        assert!(mismatch.to_string().contains("SHA-256 mismatch"));
        let mut wrong_version = release.clone();
        wrong_version.tag_name = "999.0.1-alpha.1".into();
        let mismatch = prepare(&wrong_version, "test", &mut download, &mut |_| {})
            .err()
            .unwrap();
        assert!(mismatch
            .to_string()
            .contains("did not report expected version"));
        let staged_source = prepared.source.clone();
        assert!(staged_source.join("yinz").is_file());
        drop(prepared);
        assert!(
            !staged_source.exists(),
            "aborted preparation cleans up its assets"
        );
    }
}

#[test]
fn preparation_rejects_prerelease_before_download() {
    let release = Release {
        channel: Default::default(),
        source: None,
        tag_name: "v999.0.0-rc.1".into(),
        assets: vec![],
    };
    let result = prepare(
        &release,
        "test",
        &mut |_, _| panic!("must not download"),
        &mut |_| {},
    );
    assert!(result.err().unwrap().to_string().contains("prerelease"));
}

struct StandaloneInstallation {
    _directory: tempfile::TempDir,
    source: PathBuf,
    binary: PathBuf,
    data: PathBuf,
}
impl StandaloneInstallation {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        fs::create_dir_all(source.join("plugins/builtins")).unwrap();
        fs::write(source.join("plugins/builtins/test.scm"), b"new plugin").unwrap();
        Self {
            binary: directory.path().join("yinz"),
            data: directory.path().join("data"),
            source,
            _directory: directory,
        }
    }
    fn prepare(&self) -> transaction::Transaction {
        transaction::Transaction::prepare(
            &self.source,
            &self.binary,
            &self.data,
            "coordinated-after-recovery",
        )
        .unwrap()
    }
}

#[test]
fn standalone_update_recovers_before_the_up_to_date_return() {
    for committed in [false, true] {
        let installation = StandaloneInstallation::new();
        yinz_test_support::write_warm_script(
            &installation.binary,
            "#!/bin/sh\necho 'yinz 1.0.0'\n",
        );
        yinz_test_support::write_warm_script(
            &installation.source.join("yinz"),
            "#!/bin/sh\necho 'yinz 2.0.0'\n",
        );
        let mut transaction = transaction::Transaction::prepare(
            &installation.source,
            &installation.binary,
            &installation.data,
            "standalone-interrupted",
        )
        .unwrap();
        transaction.publish().unwrap();
        if committed {
            transaction.mark_committed_for_test().unwrap();
        }
        let (binary_root, data_root) = transaction.backup_roots_for_test();
        drop(transaction);
        run_with(
            Some(&installation.data),
            &installation.binary,
            "test",
            ReleaseChannel::Stable,
            |url, output| {
                assert_eq!(url, RELEASE_URL);
                output
                    .write_all(br#"{"tag_name":"v1.0.0","assets":[]}"#)
                    .unwrap();
                Ok(())
            },
        )
        .unwrap();
        assert!(!binary_root.exists());
        assert!(!data_root.exists());
        assert_eq!(
            installed_version(&installation.binary).unwrap(),
            version(if committed { "2.0.0" } else { "1.0.0" }).unwrap()
        );
        // A subsequent coordinated operation can acquire the persistent installation claim.
        let mut next = installation.prepare();
        next.rollback().unwrap();
    }
}

#[test]
fn standalone_update_compares_the_generation_restored_by_recovery() {
    let installation = StandaloneInstallation::new();
    yinz_test_support::write_warm_script(&installation.binary, "#!/bin/sh\necho 'yinz 0.0.0'\n");
    yinz_test_support::write_warm_script(
        &installation.source.join("yinz"),
        "#!/bin/sh\necho 'yinz 999.0.0'\n",
    );
    let mut transaction = transaction::Transaction::prepare(
        &installation.source,
        &installation.binary,
        &installation.data,
        "standalone-interrupted",
    )
    .unwrap();
    transaction.publish().unwrap();
    drop(transaction);
    // The running updater's version and the published executable would both skip this
    // release. Recovery restores 0.0.0, so preparation must be attempted instead.
    let error = run_with(
        Some(&installation.data),
        &installation.binary,
        "test",
        ReleaseChannel::Stable,
        |url, output| {
            assert_eq!(url, RELEASE_URL);
            output
                .write_all(br#"{"tag_name":"v0.0.1","assets":[]}"#)
                .unwrap();
            Ok(())
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("missing"), "{error:#}");
    assert_eq!(
        installed_version(&installation.binary).unwrap(),
        version("0.0.0").unwrap()
    );
    let mut next = installation.prepare();
    next.rollback().unwrap();
}

#[test]
fn release_discovery_keeps_stable_default_and_rejects_unpublished_metadata() {
    for metadata in [
        serde_json::json!({"tag_name":"v2.0.0","assets":[]}),
        serde_json::json!({"tag_name":"v2.0.0-alpha.1","assets":[]}),
        serde_json::json!({"tag_name":"v2.0.0","assets":[],"draft":true}),
        serde_json::json!({"tag_name":"v2.0.0","assets":[],"prerelease":true}),
        serde_json::json!({"tag_name":"bad","assets":[]}),
        serde_json::json!({"tag_name":"v2.0.0","assets":[],"source":{"commit":"x","artifacts":[]}}),
    ] {
        let result = discover(ReleaseChannel::Stable, &mut |url, out| {
            assert_eq!(url, RELEASE_URL);
            out.write_all(&serde_json::to_vec(&metadata).unwrap())
                .unwrap();
            Ok(())
        });
        assert_eq!(
            result.is_ok(),
            metadata == serde_json::json!({"tag_name":"v2.0.0","assets":[]})
        );
        if let Ok(release) = result {
            assert_eq!(release.channel, ReleaseChannel::Stable);
        }
    }
}

#[test]
fn prerelease_discovery_paginates_and_uses_semver_instead_of_release_dates() {
    for (last, expected) in [("v1.0.0", "v2.0.0-alpha.10"), ("v2.0.0", "v2.0.0")] {
        let pages = [
            serde_json::json!([
                {"tag_name":"v2.0.0-alpha.9","assets":[],"created_at":"2030-01-01"},
                {"tag_name":"v99.0.0","assets":[],"draft":true}
            ]),
            serde_json::json!([
                {"tag_name":"malformed","assets":[]},
                {"tag_name":"v90.0.0","assets":[],"draft":true}
            ]),
            serde_json::json!([
                {"tag_name":"v2.0.0-alpha.10","assets":[],"created_at":"2020-01-01"},
                {"tag_name":last,"assets":[]}
            ]),
            serde_json::json!([]),
        ];
        let mut requested = 0;
        let release = discover(ReleaseChannel::Prerelease, &mut |url, out| {
            assert_eq!(
                url,
                format!("{RELEASES_URL}?per_page=100&page={}", requested + 1)
            );
            out.write_all(&serde_json::to_vec(&pages[requested]).unwrap())
                .unwrap();
            requested += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(requested, 4);
        assert_eq!(release.tag_name, expected);
        assert_eq!(release.channel, ReleaseChannel::Prerelease);
    }
}

#[test]
fn prerelease_discovery_fails_on_empty_catalog_or_failed_later_page() {
    assert!(discover(ReleaseChannel::Prerelease, &mut |_, out| {
        out.write_all(b"[]").unwrap();
        Ok(())
    })
    .unwrap_err()
    .to_string()
    .contains("no published"));
    let mut page = 0;
    let error = discover(ReleaseChannel::Prerelease, &mut |_, out| {
        page += 1;
        if page == 1 {
            out.write_all(br#"[{"tag_name":"v2.0.0-alpha.1","assets":[]}]"#)
                .unwrap();
            Ok(())
        } else {
            Err(anyhow!("rate limited"))
        }
    })
    .unwrap_err();
    assert!(error.to_string().contains("rate limited"));
}

#[test]
fn prerelease_installation_rechecks_downgrades_under_lock() {
    let installation = StandaloneInstallation::new();
    yinz_test_support::write_warm_script(&installation.binary, "#!/bin/sh\necho 'yinz 2.0.0'\n");
    yinz_test_support::write_warm_script(
        &installation.source.join("yinz"),
        "#!/bin/sh\necho 'yinz 2.0.0-alpha.1'\n",
    );
    let original = fs::read(&installation.binary).unwrap();
    let error = install_with_target(
        &installation.source,
        &installation.binary,
        &installation.data,
        Some(&version("2.0.0-alpha.1").unwrap()),
    )
    .unwrap_err();
    assert!(error.to_string().contains("refusing downgrade"));
    assert_eq!(fs::read(&installation.binary).unwrap(), original);
}
