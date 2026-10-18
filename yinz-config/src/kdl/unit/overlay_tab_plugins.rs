use crate::config::Config;

#[test]
fn defaults_replacement_empty_and_round_trip() {
    let default = Config::from_kdl("", None).unwrap();
    let tabs = default.resolved_overlay_tab_plugins().unwrap();
    assert_eq!(
        tabs.iter()
            .map(|tab| (tab.location.as_str(), tab.label.as_str()))
            .collect::<Vec<_>>(),
        vec![("yinz:agents", "Agents"), ("yinz:todos", "Todos")]
    );
    let custom = Config::from_kdl(
        r#"
        plugins { custom location="file:/custom.scm" { cwd "/work"; setting "value"; }; }
        overlay_tab_plugins { "yinz:todos"; custom label="Custom"; }
    "#,
        Some(default),
    )
    .unwrap();
    let tabs = custom.resolved_overlay_tab_plugins().unwrap();
    assert_eq!(tabs[0].label, "yinz:todos");
    assert_eq!(tabs[1].location, "file:///custom.scm");
    assert_eq!(
        tabs[1].initial_cwd.as_deref(),
        Some(std::path::Path::new("/work"))
    );
    assert_eq!(
        tabs[1].configuration.get("setting").map(String::as_str),
        Some("value")
    );
    let roundtrip = Config::from_kdl(&custom.to_string(false).unwrap(), None).unwrap();
    assert_eq!(roundtrip.overlay_tab_plugins, custom.overlay_tab_plugins);
    assert_eq!(roundtrip.resolved_overlay_tab_plugins().unwrap(), tabs);
    assert_eq!(
        Config::from_kdl("mouse_mode false", Some(custom.clone()))
            .unwrap()
            .overlay_tab_plugins,
        custom.overlay_tab_plugins
    );
    let empty = Config::from_kdl("overlay_tab_plugins {}", Some(custom)).unwrap();
    assert!(empty.resolved_overlay_tab_plugins().unwrap().is_empty());
    assert!(Config::from_kdl(&empty.to_string(false).unwrap(), None)
        .unwrap()
        .resolved_overlay_tab_plugins()
        .unwrap()
        .is_empty());
}

#[test]
fn invalid_entries_and_resolved_duplicates_have_diagnostics() {
    for text in [
        "overlay_tab_plugins { missing; }",
        "overlay_tab_plugins { \"yinz:does-not-exist\"; }",
        "overlay_tab_plugins { \"file:/plugin.wasm\"; }",
        "overlay_tab_plugins { \"https://example.com/a.scm\"; }",
        "overlay_tab_plugins { \"yinz:todos\" label=1; }",
        "overlay_tab_plugins { \"yinz:todos\" label=\"\"; }",
        "overlay_tab_plugins { \"yinz:todos\" unexpected=true; }",
        "overlay_tab_plugins { \"yinz:todos\"; \"yinz:todos\"; }",
        "plugins { todo location=\"yinz:todos\"; } overlay_tab_plugins { todo; \"yinz:todos\"; }",
        "overlay_tab_plugins {}\noverlay_tab_plugins {}",
    ] {
        assert!(Config::from_kdl(text, None).is_err(), "{text}");
    }
}

#[test]
fn editor_replaces_ordered_entries_and_preserves_alias_configuration() {
    use crate::editor;
    use yinz_types::configuration::Patch;
    let config = Config::from_kdl(
        r#"plugins { custom location="file:/custom.scm" { setting "value"; }; }"#,
        None,
    )
    .unwrap();
    let replacement = Config::from_kdl(
        r#"overlay_tab_plugins { custom label="C"; "yinz:todos" label="T"; }"#,
        Some(config.clone()),
    )
    .unwrap();
    let node = editor::document(&replacement)
        .unwrap()
        .into_iter()
        .find(|node| node.name == "overlay_tab_plugins")
        .unwrap();
    let result = editor::apply(
        &config,
        &[Patch {
            path: vec!["overlay_tab_plugins".into()],
            node: Some(node),
            reset: false,
        }],
    )
    .unwrap();
    assert_eq!(result.overlay_tab_plugins, replacement.overlay_tab_plugins);
    assert_eq!(
        result.resolved_overlay_tab_plugins().unwrap()[0].configuration["setting"],
        "value"
    );
    assert!(editor::document(&config)
        .unwrap()
        .iter()
        .any(|node| node.name == "overlay_tab_plugins" && node.children.len() == 2));
}

#[test]
fn editor_validates_alias_changes_and_overlay_changes_as_one_batch() {
    use crate::editor;
    use yinz_types::configuration::Patch;
    let base = Config::default();
    let wanted = Config::from_kdl(
        r#"
        plugins { custom location="file:/custom.scm"; }
        overlay_tab_plugins { custom; }
    "#,
        None,
    )
    .unwrap();
    let patches: Vec<_> = editor::document(&wanted)
        .unwrap()
        .into_iter()
        .filter(|node| matches!(node.name.as_str(), "plugins" | "overlay_tab_plugins"))
        .map(|node| Patch {
            path: vec![node.name.clone()],
            node: Some(node),
            reset: false,
        })
        .collect();
    let result = editor::apply(&base, &patches).unwrap();
    assert_eq!(
        result.resolved_overlay_tab_plugins().unwrap(),
        wanted.resolved_overlay_tab_plugins().unwrap()
    );
    assert!(editor::apply(
        &result,
        &[Patch {
            path: vec!["plugins".into(), "custom".into()],
            node: None,
            reset: false
        }]
    )
    .is_err());
}

#[test]
fn relative_overlay_alias_loads_from_user_plugin_directory() {
    use crate::plugins::PluginConfig;
    use yinz_runtime::plugin_location::run_plugin_from_url;

    let plugins = tempfile::tempdir().unwrap();
    let name = format!(
        "{}-overlay.scm",
        plugins.path().file_name().unwrap().to_str().unwrap()
    );
    assert!(!std::path::Path::new(&name).exists());
    let source = "(define (update event) #f)";
    std::fs::write(plugins.path().join(&name), source).unwrap();
    let config = Config::from_kdl(
        &format!(
            r#"
            plugins {{ custom location="file:{name}"; }}
            overlay_tab_plugins {{ custom; }}
        "#
        ),
        None,
    )
    .unwrap();
    let descriptor = config.resolved_overlay_tab_plugins().unwrap().remove(0);
    assert_eq!(descriptor.location, format!("file:{name}"));
    let run = run_plugin_from_url(&descriptor.location).unwrap();
    assert_eq!(
        PluginConfig::from_run_plugin(&run)
            .unwrap()
            .resolve_source(Some(plugins.path()))
            .unwrap(),
        (plugins.path().join(name), source.to_owned())
    );
}

#[test]
fn equivalent_relative_and_absolute_overlay_plugins_are_duplicates() {
    let absolute = std::env::current_dir()
        .unwrap()
        .join("overlay-duplicate.scm");
    let error = Config::from_kdl(
        &format!(
            r#"overlay_tab_plugins {{ "file:overlay-duplicate.scm"; "file:{}"; }}"#,
            absolute.display()
        ),
        None,
    )
    .unwrap_err();
    assert!(
        format!("{error:?}").contains("Duplicate resolved plugin"),
        "{error:?}"
    );
}
