use super::*;

#[test]
fn reload_matches_symlinked_plugin_directory_and_missing_paths() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let plugins = cwd.join("plugins");
    let linked = cwd.join("linked");
    std::fs::create_dir(&plugins).unwrap();
    std::os::unix::fs::symlink(&plugins, &linked).unwrap();
    let canonical = plugins.join("main.scm");
    let symlinked = linked.join("main.scm");
    std::fs::write(&canonical, "(void)").unwrap();
    let run = yinz_runtime::plugin_location::run_plugin_from_url(&format!(
        "file:{}",
        cwd.join("main.scm").display()
    ))
    .unwrap();
    let config = PluginConfig::from_run_plugin(&run).unwrap();
    let id = PluginId::new(7);
    let mut map = PluginMap::default();
    for resolved in [&canonical, &symlinked] {
        map.plugin_metadata.insert(
            id,
            PluginMetadata {
                config: config.clone(),
                data_owner: url::Url::from(&config.location),
                cache_dir: "/cache".into(),
                resolved_source_path: Some(resolved.clone()),
                cwd: cwd.clone(),
                tab_index: None,
                size: (0, 0),
                owner_client_id: ClientId::new(1),
            },
        );
        assert_eq!(
            map.update_reloaded_file_location(id),
            Some(run.location.clone())
        );
        let reloaded = map.reload_metadata(id).unwrap();
        assert_eq!(reloaded.config.path, config.path);
        assert_eq!(reloaded.resolved_source_path.as_ref(), Some(resolved));
        for location in [
            run.location.clone(),
            RunPluginLocation::File(canonical.clone()),
            RunPluginLocation::File(symlinked.clone()),
        ] {
            assert_eq!(
                map.all_plugin_ids_for_plugin_location(
                    &location,
                    &PluginUserConfiguration::default(),
                    Clone::clone
                )
                .unwrap(),
                vec![id]
            );
            assert!(map
                .all_plugin_ids_for_plugin_location(
                    &location,
                    &PluginUserConfiguration::new(std::collections::BTreeMap::from([(
                        "setting".into(),
                        "different".into()
                    )])),
                    Clone::clone
                )
                .is_err());
        }
    }
    std::fs::remove_file(&canonical).unwrap();
    for location in [run.location, RunPluginLocation::File(symlinked)] {
        assert_eq!(
            map.all_plugin_ids_for_plugin_location(
                &location,
                &PluginUserConfiguration::default(),
                Clone::clone
            )
            .unwrap(),
            vec![id]
        );
    }
    assert!(map
        .all_plugin_ids_for_plugin_location(
            &RunPluginLocation::File(cwd.join("other.scm")),
            &PluginUserConfiguration::default(),
            Clone::clone
        )
        .is_err());
}
