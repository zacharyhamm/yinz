use super::*;
use yinz_types::input::{git_plugin::GitPlugin, layout::RunPlugin};

#[test]
fn paths_aliases_and_missing_sources_resolve_without_loading() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    std::fs::create_dir(cwd.join("nested")).unwrap();
    let path = cwd.join("nested/main.ts");
    std::fs::write(&path, "").unwrap();
    std::os::unix::fs::symlink(cwd.join("nested"), cwd.join("linked")).unwrap();
    let repository = GitPlugin {
        repo: "https://example.com/a.git".into(),
        entrypoint: "main.ts".into(),
        revision: None,
    };
    let aliases = PluginAliases::from_data(
        [
            (
                "file".into(),
                RunPlugin {
                    location: RunPluginLocation::File(path.clone()),
                    ..Default::default()
                },
            ),
            (
                "repo".into(),
                RunPlugin {
                    location: RunPluginLocation::Git(repository.clone()),
                    ..Default::default()
                },
            ),
            (
                "builtin".into(),
                RunPlugin {
                    location: RunPluginLocation::Yinz(yinz_types::data::PluginTag::new("agent")),
                    ..Default::default()
                },
            ),
        ]
        .into(),
    );
    for name in [
        path.to_str().unwrap(),
        "./nested/main.ts",
        "../nested/main.ts",
        "./linked/main.ts",
        "file:./nested/main.ts",
        "file",
    ] {
        let base = if name.starts_with("../") {
            cwd.join("nested")
        } else {
            cwd.clone()
        };
        assert!(
            matches!(resolve_log_target(name, None, &cwd, &aliases, &base).unwrap(), LogTarget::File(found) if found == path),
            "{name}"
        );
    }
    assert!(
        matches!(resolve_log_target("repo", None, &cwd, &aliases, &cwd).unwrap(), LogTarget::Repository(found) if found == repository.descriptor())
    );
    assert!(
        matches!(resolve_log_target("builtin", None, &cwd, &aliases, &cwd).unwrap(), LogTarget::Builtin(found) if found == "agent")
    );
    assert!(
        matches!(resolve_log_target("nested/main", Some(&cwd), &cwd, &aliases, &cwd).unwrap(), LogTarget::Legacy(found) if found == path)
    );
    std::fs::remove_file(&path).unwrap();
    assert!(
        matches!(resolve_log_target("./linked/main.ts", None, &cwd, &aliases, &cwd).unwrap(), LogTarget::File(found) if found == path)
    );
    assert!(
        matches!(resolve_log_target("./absent/../main.ts", None, &cwd, &aliases, &cwd).unwrap(), LogTarget::File(found) if found == cwd.join("main.ts"))
    );
}

#[test]
fn exact_identity_isolates_same_names_and_repository_revisions() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let repository = GitPlugin {
        repo: "https://example.com/a.git".into(),
        entrypoint: "main.ts".into(),
        revision: None,
    };
    let other = GitPlugin {
        repo: "https://example.com/b.git".into(),
        ..repository.clone()
    };
    let first = cwd.join("rev1/main.ts");
    let second = cwd.join("rev2/main.ts");
    let marker = |repo: &GitPlugin, path: &Path| {
        LogIdentity::new(&RunPluginLocation::Git(repo.clone()), path, &cwd).marker()
    };
    let one = format!("{} [deno:main.ts] console", marker(&repository, &first));
    let two = format!(
        "{} [deno:main.ts] worker",
        marker(&other, &cwd.join("other/main.ts"))
    );
    let three = format!("{} runtime error", marker(&repository, &second));
    let log = cwd.join("log");
    std::fs::write(&log, format!("old main.ts\n{one}\n{two}\n{three}\n")).unwrap();
    assert_eq!(
        log_target_tail(&log, &LogTarget::File(first), 10).unwrap(),
        format!("{one}\n")
    );
    assert_eq!(
        log_target_tail(&log, &LogTarget::Repository(repository.descriptor()), 10).unwrap(),
        format!("{one}\n{three}\n")
    );
    assert_eq!(
        log_target_tail(&log, &LogTarget::Repository(repository.descriptor()), 1).unwrap(),
        format!("{three}\n")
    );
    assert!(
        log_target_tail(&cwd.join("missing"), &LogTarget::File(second), 10)
            .unwrap_err()
            .to_string()
            .contains("no yinz log")
    );
}

#[test]
fn escaped_identity_survives_log_framing() {
    let path = Path::new("/external/line\n\"[x]/main.wasm");
    let identity = LogIdentity::new(&RunPluginLocation::File(path.into()), path, Path::new("/"));
    let marker = identity.marker();
    assert!(!marker.contains('\n'));
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("log");
    std::fs::write(&log, format!("{marker} stderr\nold main.wasm\n")).unwrap();
    assert_eq!(
        log_target_tail(&log, &LogTarget::File(path.into()), 0).unwrap(),
        format!("{marker} stderr\n")
    );
}

#[test]
fn builtin_aliases_exclude_legacy_and_bare_names_ignore_other_sources_messages() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("log");
    let builtin = LogIdentity::new(
        &RunPluginLocation::Yinz(yinz_types::data::PluginTag::new("agent-policy")),
        Path::new("agent-policy"),
        dir.path(),
    )
    .marker();
    let file = LogIdentity::new(
        &RunPluginLocation::File("/p/other.ts".into()),
        Path::new("/p/other.ts"),
        dir.path(),
    )
    .marker();
    let entry = format!("{builtin} [deno:agent-policy] new");
    std::fs::write(
        &log,
        format!("[deno:agent-policy] old\n{entry}\n{file} mentions agent-policy.ts\n"),
    )
    .unwrap();
    assert_eq!(
        log_target_tail(&log, &LogTarget::Builtin("agent-policy".into()), 10).unwrap(),
        format!("{entry}\n")
    );
    assert_eq!(
        crate::log_tail(&log, Path::new("/p/agent-policy.ts"), 10).unwrap(),
        format!("[deno:agent-policy] old\n{entry}\n")
    );
}

#[test]
fn non_utf8_source_identity_is_lossless() {
    use std::os::unix::ffi::OsStringExt;
    let path = PathBuf::from(std::ffi::OsString::from_vec(b"/p/\xff/main.ts".to_vec()));
    let other = Path::new("/p/\u{fffd}/main.ts");
    let identity = LogIdentity::new(
        &RunPluginLocation::File(path.clone()),
        &path,
        Path::new("/"),
    );
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("log");
    std::fs::write(&log, identity.marker() + " line\n").unwrap();
    assert!(!log_target_tail(&log, &LogTarget::File(path), 1)
        .unwrap()
        .is_empty());
    assert!(log_target_tail(&log, &LogTarget::File(other.into()), 1)
        .unwrap()
        .is_empty());
}

#[test]
fn file_aliases_select_the_loaders_fallback_and_preferred_sources() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let cwd = root.join("cwd");
    let scripts = root.join("scripts");
    let data = root.join("data");
    for directory in [&cwd, &scripts, &data.join("plugins")] {
        std::fs::create_dir_all(directory).unwrap();
    }
    for (name, fallback) in [
        ("keys.ts", scripts.join("keys.ts")),
        ("keys", data.join("plugins/keys.wasm")),
    ] {
        let (location, relative_path) =
            yinz_runtime::plugin_location::parse_plugin_location_with_relative(
                &format!("file:{name}"),
                Some(cwd.clone()),
            )
            .unwrap();
        let plugin = RunPlugin {
            location,
            relative_path,
            ..Default::default()
        };
        let config = PluginConfig::from_run_plugin(&plugin).unwrap();
        let aliases = PluginAliases::from_data([("alias".into(), plugin)].into());
        std::fs::write(&fallback, "fallback").unwrap();
        let preferred =
            cwd.join(name)
                .with_extension(if name.ends_with(".ts") { "ts" } else { "wasm" });
        for expected in [&fallback, &preferred] {
            std::fs::write(expected, "source").unwrap();
            let loaded = match config.runtime() {
                PluginRuntime::Deno => config.resolve_source(Some(&scripts)).unwrap().0,
                PluginRuntime::Wasm => config.resolve_wasm_source(&data, None).unwrap().0,
            };
            assert_eq!(&loaded, expected);
            let target =
                resolve_log_target("alias", Some(&scripts), &data, &aliases, &cwd).unwrap();
            assert!(matches!(&target, LogTarget::File(path) if path == &loaded));
            let selected = LogIdentity::new(&config.location, &loaded, &cwd).marker() + " selected";
            let unrelated = LogIdentity::new(&config.location, &root.join(name), &cwd).marker();
            let log = root.join("log");
            std::fs::write(&log, format!("{selected}\n{unrelated} unrelated\n")).unwrap();
            assert_eq!(log_target_tail(&log, &target, 10).unwrap(), selected + "\n");
        }
        std::fs::remove_file(&fallback).unwrap();
        std::fs::remove_file(&preferred).unwrap();
        assert!(matches!(
            resolve_log_target("alias", Some(&scripts), &data, &aliases, &cwd).unwrap(),
            LogTarget::File(path) if path == cwd.join(name)
        ));
    }
}

#[test]
fn plugin_cli_explicit_targets_match_log_resolution() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let path = cwd.join("space name.ts");
    for target in [
        path.display().to_string(),
        "./space name.ts".into(),
        format!(
            "../{}/space name.ts",
            cwd.file_name().unwrap().to_str().unwrap()
        ),
        format!("file:{}", path.display()),
        format!("file://{}", path.display()),
        format!("file://{}", path.display()).replace(' ', "%20"),
        "file:./space name.ts".into(),
    ] {
        let resolved = resolve_explicit_plugin_path(&target, &cwd)
            .unwrap()
            .unwrap();
        assert_eq!(source_path(&resolved, &cwd), path, "{target}");
        assert!(
            matches!(resolve_log_target(&target, None, &cwd, &PluginAliases::default(), &cwd).unwrap(), LogTarget::File(found) if found == path)
        );
    }
    for name in ["foo", "foo.ts", "folder/foo", " space.ts "] {
        assert_eq!(resolve_explicit_plugin_path(name, &cwd).unwrap(), None);
    }
    assert_eq!(
        resolve_explicit_plugin_path("./ missing.ts ", &cwd).unwrap(),
        Some(cwd.join(" missing.ts "))
    );
}
