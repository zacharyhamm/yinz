use super::*;

#[test]
fn file_plugin_paths_survive_config_serialization() {
    use crate::plugins::PluginConfig;
    use yinz_types::input::hooks::HookPoint;

    let directory = tempfile::tempdir().unwrap();
    let name = format!(
        "{} # %20?.scm",
        directory.path().file_name().unwrap().to_str().unwrap()
    );
    assert!(!std::env::current_dir().unwrap().join(&name).exists());
    let source = "(void)";
    std::fs::write(directory.path().join(&name), source).unwrap();
    let mut urls = vec![(format!("file:{name}"), directory.path().join(&name), true)];
    for name in [
        "hash#.scm",
        "percent%20.scm",
        "query?.scm",
        "space name.scm",
    ] {
        let path = directory.path().join(name);
        std::fs::write(&path, source).unwrap();
        urls.push((
            url::Url::from_file_path(&path).unwrap().to_string(),
            path,
            false,
        ));
    }
    #[cfg(unix)]
    {
        let path = directory.path().join("current # %20?.scm");
        std::os::unix::fs::symlink(directory.path().join(&name), &path).unwrap();
        urls.push((
            url::Url::from_file_path(&path).unwrap().to_string(),
            path,
            false,
        ));
    }
    for (url, expected_path, fallback) in urls {
        let text = format!(
            r#"
        plugins {{ panel location="{url}"; }}
        load_plugins clear-defaults=true {{ "{url}"; }}
        hooks {{ agent_exit "{url}"; }}
        keybinds {{ normal {{ bind "Ctrl o" {{
            LaunchPlugin "{url}"
            LaunchOrFocusPlugin "{url}"
        }}; }}; }}
    "#
        );
        let config = Config::from_kdl(&text, None).unwrap();
        let restored = Config::from_kdl(&config.to_string(false).unwrap(), None).unwrap();
        for config in [config, restored] {
            let mut plugins = vec![config.plugins.aliases["panel"].clone()];
            plugins.extend(
                config
                    .background_plugins
                    .iter()
                    .filter_map(|p| p.get_run_plugin()),
            );
            plugins.push(
                config
                    .hooks
                    .get(HookPoint::AgentExit)
                    .unwrap()
                    .get_run_plugin()
                    .unwrap(),
            );
            let key = KeyWithModifier::new(BareKey::Char('o')).with_ctrl_modifier();
            for action in config
                .keybinds
                .get_actions_for_key_in_mode(&InputMode::Normal, &key)
                .unwrap()
            {
                let (Action::LaunchPlugin { plugin, .. }
                | Action::LaunchOrFocusPlugin { plugin, .. }) = action
                else {
                    panic!("plugin action")
                };
                plugins.push(plugin.get_run_plugin().unwrap());
            }
            assert_eq!(plugins.len(), 5);
            for plugin in plugins {
                assert_eq!(
                    plugin.relative_path.as_deref(),
                    fallback.then(|| std::path::Path::new(&name))
                );
                if !fallback {
                    assert_eq!(plugin.declared_path.as_ref(), Some(&expected_path));
                }
                assert_eq!(
                    PluginConfig::from_run_plugin(&plugin)
                        .unwrap()
                        .resolve_source(Some(directory.path()))
                        .unwrap(),
                    (expected_path.clone(), source.to_owned())
                );
            }
        }
    }
}

#[test]
fn plugins_to_string() {
    let fake_config = r##"
        plugins {
            tab-bar location="yinz:tab-bar"
            status-bar location="yinz:status-bar"
            strider location="yinz:strider"
            compact-bar location="yinz:compact-bar"
            session-palette location="yinz:session-palette"
            session-browser location="yinz:session-palette" {
                shared true
            }
            filepicker location="yinz:strider" {
                cwd "/"
            }
        }"##;
    let document: KdlDocument = fake_config.parse().unwrap();
    let deserialized =
        crate::kdl::plugin_aliases::from_kdl(document.get("plugins").unwrap()).unwrap();
    let serialized = crate::kdl::plugin_aliases::to_kdl(&deserialized, true);
    let deserialized_from_serialized = crate::kdl::plugin_aliases::from_kdl(
        serialized
            .to_string()
            .parse::<KdlDocument>()
            .unwrap()
            .get("plugins")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        deserialized, deserialized_from_serialized,
        "Deserialized serialized config equals original config"
    );
    insta::assert_snapshot!(serialized.to_string());
}

#[test]
fn plugins_to_string_with_file() {
    let fake_config = r##"
        plugins {
            tab-bar location="yinz:tab-bar"
            filepicker location="file:/path/to/my/plugin.scm" {
                cwd "/"
            }
        }"##;
    let document: KdlDocument = fake_config.parse().unwrap();
    let deserialized =
        crate::kdl::plugin_aliases::from_kdl(document.get("plugins").unwrap()).unwrap();
    let serialized = crate::kdl::plugin_aliases::to_kdl(&deserialized, true);
    let deserialized_from_serialized = crate::kdl::plugin_aliases::from_kdl(
        serialized
            .to_string()
            .parse::<KdlDocument>()
            .unwrap()
            .get("plugins")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        deserialized, deserialized_from_serialized,
        "Deserialized serialized config equals original config"
    );
    insta::assert_snapshot!(serialized.to_string());
}

#[test]
fn ui_config_to_string() {
    let fake_config = r##"
        ui {
            pane_frames {
                rounded_corners true
                hide_session_name true
            }
        }"##;
    let document: KdlDocument = fake_config.parse().unwrap();
    let deserialized = UiConfig::from_kdl(document.get("ui").unwrap()).unwrap();
    let serialized = UiConfig::to_kdl(&deserialized).unwrap();
    let deserialized_from_serialized = UiConfig::from_kdl(
        serialized
            .to_string()
            .parse::<KdlDocument>()
            .unwrap()
            .get("ui")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        deserialized, deserialized_from_serialized,
        "Deserialized serialized config equals original config"
    );
    insta::assert_snapshot!(serialized.to_string());
}

#[test]
fn ui_config_to_string_with_no_ui_config() {
    let fake_config = r##"
        ui {
            pane_frames {
            }
        }"##;
    let document: KdlDocument = fake_config.parse().unwrap();
    let deserialized = UiConfig::from_kdl(document.get("ui").unwrap()).unwrap();
    assert_eq!(UiConfig::to_kdl(&deserialized), None);
}

#[test]
fn env_vars_to_string() {
    let fake_config = r##"
        env {
            foo "bar"
            bar "foo"
            thing 1
            baz "true"
        }"##;
    let document: KdlDocument = fake_config.parse().unwrap();
    let deserialized = environment_variables_from_kdl(document.get("env").unwrap()).unwrap();
    let serialized = environment_variables_to_kdl(&deserialized).unwrap();
    let deserialized_from_serialized = environment_variables_from_kdl(
        serialized
            .to_string()
            .parse::<KdlDocument>()
            .unwrap()
            .get("env")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        deserialized, deserialized_from_serialized,
        "Deserialized serialized config equals original config"
    );
    insta::assert_snapshot!(serialized.to_string());
}

#[test]
fn env_vars_to_string_with_no_env_vars() {
    let fake_config = r##"
        env {
        }"##;
    let document: KdlDocument = fake_config.parse().unwrap();
    let deserialized = environment_variables_from_kdl(document.get("env").unwrap()).unwrap();
    assert_eq!(environment_variables_to_kdl(&deserialized), None);
}
