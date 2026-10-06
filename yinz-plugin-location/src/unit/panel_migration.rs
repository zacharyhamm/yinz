use super::*;
use yinz_types::pane_tabs::PaneTabDescriptor;

#[test]
fn legacy_panel_locations_and_aliases_normalize_after_configuration_merging() {
    let inherited = BTreeMap::from([
        ("view".into(), "vms".into()),
        ("shared".into(), "true".into()),
        ("keep".into(), "inherited".into()),
    ]);
    let aliases = PluginAliases {
        aliases: BTreeMap::from([(
            "legacy".into(),
            run_plugin_from_url("yinz:agents")
                .unwrap()
                .with_configuration(inherited.clone())
                .with_initial_cwd(Some("/alias".into())),
        )]),
    };
    for (view, expected) in [("vms", "yinz:vms"), ("swamp", "yinz:swamp-issues")] {
        let configuration = BTreeMap::from([
            ("view".into(), view.into()),
            ("keep".into(), "override".into()),
        ]);
        for location in ["yinz:agents", "legacy"] {
            let mut run =
                run_plugin_or_alias_from_url(location, &Some(configuration.clone()), None, None)
                    .unwrap()
                    .with_initial_cwd(Some("/origin".into()));
            run.populate_run_plugin_if_needed(&aliases);
            let plugin = run.get_run_plugin().unwrap();
            assert_eq!(plugin.location.display(), expected);
            assert!(!plugin.configuration.inner().contains_key("view"));
            assert_eq!(plugin.configuration.inner()["keep"], "override");
            assert_eq!(plugin.initial_cwd, Some("/origin".into()));
            if location == "legacy" {
                assert_eq!(plugin.configuration.inner()["shared"], "true");
            }
            let mut descriptor = PaneTabDescriptor {
                label: "Custom label".into(),
                location: location.into(),
                configuration: configuration.clone(),
                initial_cwd: Some("/origin".into()),
            };
            descriptor.populate_plugin_alias(&aliases);
            assert_eq!(descriptor.location, expected);
            assert_eq!(descriptor.configuration, *plugin.configuration.inner());
            assert_eq!(descriptor.initial_cwd, plugin.initial_cwd);
            assert_eq!(descriptor.label, "Custom label");
            let serialized = serde_json::to_string(&descriptor).unwrap();
            let mut restored: PaneTabDescriptor = serde_json::from_str(&serialized).unwrap();
            restored.populate_plugin_alias(&aliases);
            assert_eq!(restored, descriptor);
        }
    }
    let plugin = run_plugin_or_alias_from_url("legacy", &None, Some(&aliases), None)
        .unwrap()
        .get_run_plugin()
        .unwrap();
    assert_eq!(plugin.location.display(), "yinz:vms");
    assert_eq!(plugin.initial_cwd, Some("/alias".into()));
}

#[test]
fn custom_plugins_and_agents_child_markers_are_preserved() {
    for location in ["file:/tmp/agents.wasm", "yinz:agents"] {
        for view in ["agents", "custom", "vms", "swamp"] {
            if location == "yinz:agents" && matches!(view, "vms" | "swamp") {
                continue;
            }
            let configuration = BTreeMap::from([("view".into(), view.into())]);
            let mut run =
                run_plugin_or_alias_from_url(location, &Some(configuration.clone()), None, None)
                    .unwrap();
            run.populate_run_plugin_if_needed(&PluginAliases::default());
            let plugin = run.get_run_plugin().unwrap();
            assert_eq!(plugin.location.display(), location);
            assert_eq!(plugin.configuration.inner(), &configuration);
            let mut descriptor = PaneTabDescriptor {
                label: "Original".into(),
                location: location.into(),
                configuration: configuration.clone(),
                initial_cwd: Some("/origin".into()),
            };
            let original = descriptor.clone();
            descriptor.populate_plugin_alias(&PluginAliases::default());
            assert_eq!(descriptor, original);
        }
    }
}

#[test]
fn custom_file_alias_descriptors_keep_their_location_and_view() {
    let aliases = PluginAliases {
        aliases: BTreeMap::from([(
            "custom".into(),
            run_plugin_from_url("file:/tmp/agents.wasm")
                .unwrap()
                .with_configuration(BTreeMap::from([("view".into(), "vms".into())])),
        )]),
    };
    let mut tab = PaneTabDescriptor {
        label: "Custom".into(),
        location: "custom".into(),
        configuration: BTreeMap::from([("view".into(), "swamp".into())]),
        initial_cwd: Some("/origin".into()),
    };
    let original = tab.clone();
    tab.populate_plugin_alias(&aliases);
    assert_eq!(tab, original);
}
