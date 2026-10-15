use super::*;
use std::collections::{BTreeSet, HashSet};

#[test]
fn file_identity_is_resolved_once_and_preserves_relative_fallback() {
    let root = std::env::temp_dir().join(format!("yinz-plugin-identity-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("plugin.scm");
    std::fs::write(&path, "(void)").unwrap();
    let canonical = path.canonicalize().unwrap();
    let (location, relative) =
        parse_plugin_location_with_relative("file:plugin.scm", Some(root.clone())).unwrap();
    assert_eq!(relative, Some(PathBuf::from("plugin.scm")));
    assert_eq!(location, RunPluginLocation::File(canonical.clone()));
    let relative_plugin =
        run_plugin_from_url_with_cwd("file:plugin.scm", Some(root.clone())).unwrap();
    assert_eq!(relative_plugin.declared_path, Some(path.clone()));
    let absolute_plugin = run_plugin_from_url(&format!("file:{}", path.display())).unwrap();
    let mut plugins = vec![relative_plugin.clone(), absolute_plugin];
    #[cfg(unix)]
    {
        let link = root.join("link.scm");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        plugins.push(run_plugin_from_url(&format!("file:{}", link.display())).unwrap());
    }
    assert!(plugins.iter().all(|plugin| plugin == &relative_plugin));
    assert_eq!(plugins.iter().collect::<HashSet<_>>().len(), 1);
    assert_eq!(plugins.iter().collect::<BTreeSet<_>>().len(), 1);
    for plugin in &plugins {
        assert_eq!(
            plugin.location_for_serialization(),
            Url::from_file_path(plugin.declared_path.as_ref().unwrap())
                .unwrap()
                .to_string()
        );
    }
    let mut configured = relative_plugin.clone();
    configured.configuration.insert("key", "value");
    assert_ne!(configured, relative_plugin);
    assert_ne!(configured.cmp(&relative_plugin), std::cmp::Ordering::Equal);

    let cwd = std::env::current_dir().unwrap();
    assert_eq!(
        parse_plugin_location("file:", None).unwrap(),
        RunPluginLocation::File(cwd.canonicalize().unwrap())
    );
    let current = parse_plugin_location("file:Cargo.toml", None).unwrap();
    assert_eq!(
        current,
        RunPluginLocation::File(cwd.join("Cargo.toml").canonicalize().unwrap())
    );
    let (missing, relative) =
        parse_plugin_location_with_relative("file:missing.scm", Some(root.clone())).unwrap();
    assert_eq!(missing, RunPluginLocation::File(root.join("missing.scm")));
    assert_eq!(relative, Some(PathBuf::from("missing.scm")));
    let missing_plugin = RunPlugin {
        location: missing,
        relative_path: relative,
        ..Default::default()
    };
    assert_eq!(
        missing_plugin.location_for_serialization(),
        "file:missing.scm"
    );
    let absolute_missing = format!("file:{}", root.join("missing.scm").display());
    let explicit = run_plugin_from_url(&absolute_missing).unwrap();
    assert_eq!(explicit.relative_path, None);
    assert_eq!(
        explicit.location_for_serialization(),
        Url::from_file_path(root.join("missing.scm"))
            .unwrap()
            .to_string()
    );
    assert_eq!(
        parse_plugin_location("file:missing.scm", None).unwrap(),
        RunPluginLocation::File(cwd.join("missing.scm"))
    );
    assert_eq!(
        parse_plugin_location("file:missing.scm", Some(PathBuf::from("subdir"))).unwrap(),
        RunPluginLocation::File(cwd.join("subdir/missing.scm"))
    );

    #[cfg(unix)]
    {
        let loop_path = root.join("loop.scm");
        std::os::unix::fs::symlink(&loop_path, &loop_path).unwrap();
        assert!(loop_path.try_exists().is_err());
        let (location, relative_path) =
            parse_plugin_location_with_relative("file:loop.scm", Some(root.clone())).unwrap();
        let plugin = RunPlugin {
            location,
            relative_path,
            ..Default::default()
        };
        assert_eq!(
            plugin.location_for_serialization(),
            Url::from_file_path(&loop_path).unwrap().to_string()
        );
    }

    // Equality and ordering do not touch the filesystem after parsing.
    std::fs::remove_dir_all(&root).unwrap();
    assert_eq!(plugins.iter().collect::<HashSet<_>>().len(), 1);
    assert_eq!(plugins.iter().collect::<BTreeSet<_>>().len(), 1);
}

#[test]
fn old_serialized_plugins_keep_their_loading_path() {
    let plugin: RunPlugin = serde_json::from_value(serde_json::json!({
        "location": {"File": "/somewhere/plugin # %20?.ts"},
        "configuration": {},
        "initial_cwd": null
    }))
    .unwrap();
    assert_eq!(plugin.declared_path, None);
    assert_eq!(plugin.relative_path, None);
    let restored = run_plugin_from_url(&plugin.location_for_serialization()).unwrap();
    assert_eq!(restored.location, plugin.location);
    assert_eq!(
        restored.declared_path,
        Some(PathBuf::from("/somewhere/plugin # %20?.ts"))
    );
}

#[test]
#[cfg(unix)]
fn focus_compares_canonical_identity_after_retargeting_a_declaration() {
    use yinz_types::input::layout::Run;

    let root = std::env::temp_dir().join(format!("yinz-plugin-focus-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let first = root.join("v1.scm");
    let second = root.join("v2.scm");
    let current = root.join("current.scm");
    for path in [&first, &second] {
        std::fs::write(path, "(void)").unwrap();
    }
    std::os::unix::fs::symlink(&first, &current).unwrap();
    let url = Url::from_file_path(&current).unwrap().to_string();
    let running = run_plugin_from_url(&url).unwrap();
    let mut stored = vec![
        RunPluginOrAlias::RunPlugin(running.clone()),
        RunPluginOrAlias::Alias(PluginAlias {
            name: "current".into(),
            run_plugin: Some(running.clone()),
            ..Default::default()
        }),
        // Older records have only a file location to refresh.
        RunPluginOrAlias::RunPlugin(RunPlugin {
            location: RunPluginLocation::File(current.clone()),
            ..Default::default()
        }),
    ];
    std::fs::remove_file(&current).unwrap();
    std::os::unix::fs::symlink(&second, &current).unwrap();
    let requested = run_plugin_from_url(&url).unwrap();
    let matches = |requested: &RunPlugin, running: &RunPlugin, expected| {
        let run = Some(Run::Plugin(RunPluginOrAlias::RunPlugin(running.clone())));
        for request in [
            RunPluginOrAlias::RunPlugin(requested.clone()),
            RunPluginOrAlias::Alias(PluginAlias {
                name: "current".into(),
                run_plugin: Some(requested.clone()),
                ..Default::default()
            }),
        ] {
            assert_eq!(request.is_equivalent_to_run(&run), expected);
        }
    };
    for request in &mut stored {
        refresh_file_identity(request);
        let refreshed = request.get_run_plugin().unwrap();
        assert_eq!(refreshed, requested);
        matches(&refreshed, &running, false);
        matches(&refreshed, &requested, true);
        let mut configured = requested.clone();
        configured.configuration.insert("key", "different");
        matches(&refreshed, &configured, false);
    }
    matches(&requested, &running, false);
    matches(&running, &requested, false);
    let mut different = requested.clone();
    different.configuration.insert("key", "value");
    matches(&different, &running, false);
    different = requested.clone();
    different.declared_path = Some(root.join("unrelated.scm"));
    matches(&different, &running, false);
    different.location = running.location.clone();
    matches(&different, &running, true);
    for declared_path in [None, Some(PathBuf::new())] {
        let mut old = running.clone();
        old.declared_path = declared_path.clone();
        different = requested.clone();
        different.declared_path = declared_path;
        matches(&different, &old, false);
    }
    different = requested.clone();
    different.location = RunPluginLocation::Yinz(PluginTag::new("strider"));
    matches(&different, &running, false);

    std::fs::remove_dir_all(root).unwrap();
    // A missing declaration retains its spelling, not the previously canonical target.
    for request in &mut stored {
        let previous = request.get_run_plugin().unwrap();
        let expected = previous
            .declared_path
            .clone()
            .map(RunPluginLocation::File)
            .unwrap_or(previous.location);
        refresh_file_identity(request);
        assert_eq!(request.get_run_plugin().unwrap().location, expected);
    }
    matches(&requested, &running, false);
    let mut reloaded = running.clone();
    reloaded.location = requested.location.clone();
    matches(&requested, &reloaded, true);
    matches(&running, &reloaded, false);
    assert_ne!(requested, running);
    assert_eq!(
        [&requested, &running]
            .into_iter()
            .collect::<HashSet<_>>()
            .len(),
        2
    );
    assert_eq!(
        [&requested, &running]
            .into_iter()
            .collect::<BTreeSet<_>>()
            .len(),
        2
    );
}
