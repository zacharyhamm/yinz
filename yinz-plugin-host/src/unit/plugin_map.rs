use super::configuration_matches_for_reload;
use yinz_types::input::layout::PluginUserConfiguration;

#[test]
fn reload_metadata_lookup_needs_no_guest_and_deduplicates_clients() {
    use super::{ClientId, PluginConfig, PluginId, PluginMap, RunPluginLocation};
    let mut map = PluginMap::default();
    for (id, extension) in [(7, "scm"), (8, "ss"), (9, "sld")] {
        let run = yinz_runtime::plugin_location::run_plugin_from_url(&format!(
            "file:/cwd/plugin.{extension}"
        ))
        .unwrap();
        let config = PluginConfig::from_run_plugin(&run).unwrap();
        let resolved = std::path::PathBuf::from(format!("/plugins/plugin.{extension}"));
        // No runtime assets: every lookup below must be served from cached metadata.
        for client in [1, 2] {
            map.plugin_metadata.insert(
                PluginId::new(id),
                super::PluginMetadata {
                    config: config.clone(),
                    data_owner: url::Url::from(&config.location),
                    resolved_source_path: Some(resolved.clone()),
                    cache_dir: "/cache".into(),
                    cwd: "/cwd".into(),
                    tab_index: None,
                    size: (0, 0),
                    owner_client_id: ClientId::new(client),
                },
            );
        }
        assert_eq!(
            map.all_plugin_ids_for_plugin_location(
                &RunPluginLocation::File(resolved),
                &PluginUserConfiguration::default(),
                Clone::clone,
            )
            .unwrap(),
            vec![PluginId::new(id)]
        );
        assert_eq!(
            map.run_plugin_of_plugin_id(PluginId::new(id))
                .unwrap()
                .location,
            run.location
        );
        assert_eq!(
            map.run_plugin_of_plugin_id(PluginId::new(id))
                .unwrap()
                .declared_path,
            run.declared_path
        );
        assert_eq!(
            map.plugin_config_of_plugin_id(PluginId::new(id)),
            Some(config)
        );
    }
    assert_eq!(map.clone_plugin_assets().len(), 3);
}

#[test]
fn plugin_failure_identity_rejects_replacements_and_disconnected_clients() {
    use super::{Arc, AtomicBool, ClientId, Ordering, PluginId, PluginMap};
    let mut map = PluginMap::default();
    let id = PluginId::new(7);
    let client = ClientId::new(1);
    let cancel = Arc::new(AtomicBool::new(false));
    // Metadata alone is sufficient: no running-plugin lock or guest runtime is needed.
    map.client_cancellations
        .insert((id, client), (Some(41), cancel.clone()));
    assert!(map.contains_generation(id, 41));
    map.client_cancellations
        .insert((id, client), (Some(42), cancel.clone()));
    assert!(!map.contains_generation(id, 41));
    assert!(map.contains_generation(id, 42));
    let peer = Arc::new(AtomicBool::new(false));
    map.client_cancellations
        .insert((id, ClientId::new(2)), (Some(43), peer.clone()));
    cancel.store(true, Ordering::Release);
    assert!(!map.contains_generation(id, 42));
    assert!(map.contains_generation(id, 43));
    assert!(!map.contains_generation(PluginId::new(8), 43));
    map.invalidate_generations(id);
    assert!(!map.contains_generation(id, 43));
    // Reload invalidates failure identity, but a subsequent disconnect must still
    // reach the exact attachment token consulted by contains().
    let (generation, retained_cancel) = &map.client_cancellations[&(id, ClientId::new(2))];
    assert_eq!(*generation, None);
    assert!(Arc::ptr_eq(retained_cancel, &peer));
    peer.store(true, Ordering::Release);
    assert!(retained_cancel.load(Ordering::Acquire));
    map.client_cancellations.clear();
    assert!(!map.contains_generation(id, 43));
}

fn configuration(entries: &[(&str, &str)]) -> PluginUserConfiguration {
    PluginUserConfiguration::new(
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    )
}

#[test]
fn reload_lookup_matches_a_configured_plugin() {
    // eg. agent launched from an alias, then reloaded by its bare location
    let running = configuration(&[
        ("dobbs_path", "/home/bob/dobbs"),
        ("shared", "true"),
        ("caller_cwd", "/home/bob"),
        ("caller_terminal_pane_id", "7"),
    ]);

    // no configuration at all: "reload whatever runs at this location"
    assert!(configuration_matches_for_reload(
        &running,
        &configuration(&[])
    ));
    // only caller_cwd differs, which is per-launch and not part of the identity
    assert!(configuration_matches_for_reload(
        &running,
        &configuration(&[
            ("dobbs_path", "/home/bob/dobbs"),
            ("shared", "true"),
            ("caller_cwd", "/tmp"),
            ("caller_terminal_pane_id", "9"),
        ])
    ));
    // an explicit, differing configuration is still a different plugin
    assert!(!configuration_matches_for_reload(
        &running,
        &configuration(&[("dobbs_path", "/other/dobbs"), ("shared", "true")])
    ));
}
