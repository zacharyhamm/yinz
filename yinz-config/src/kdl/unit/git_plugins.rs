use crate::config::Config;
use yinz_types::input::{git_plugin::GitRevision, layout::RunPluginLocation};

#[test]
fn repository_aliases_validate_and_round_trip_without_leaking_properties() {
    let text = r#"plugins {
        remote repo="git@example.com:team/plugin.git" branch="main" entrypoint="src/panel.scm" { greeting "Hello"; }
        local repo="./tools"
    }"#;
    let config = Config::from_kdl(text, None).unwrap();
    let remote = &config.plugins.aliases["remote"];
    assert_eq!(remote.configuration.inner().len(), 1);
    assert_eq!(remote.configuration.inner()["greeting"], "Hello");
    let RunPluginLocation::Git(plugin) = &remote.location else {
        panic!("Git location")
    };
    assert_eq!(plugin.revision, Some(GitRevision::Branch("main".into())));
    assert_eq!(plugin.entrypoint, std::path::Path::new("src/panel.scm"));
    let RunPluginLocation::Git(local) = &config.plugins.aliases["local"].location else {
        panic!("local Git location")
    };
    assert_eq!(local.entrypoint, std::path::Path::new("main.scm"));
    let node = crate::kdl::plugin_aliases::to_kdl(&config.plugins, false);
    assert_eq!(
        crate::kdl::plugin_aliases::from_kdl(&node).unwrap(),
        config.plugins
    );
    for declaration in [
        "repo=\"https://example.com/p.git\" location=\"file:main.scm\"",
        "repo=\"https://example.com/p.git\" branch=\"main\" tag=\"v1\"",
        "repo=\"./local\" branch=\"main\"",
        "repo=\"file:tools\" branch=\"main\"",
        "repo=\"file:tools\" entrypoint=\"../escape.scm\"",
        "repo=\"https://example.com/p.git\" entrypoint=\"../escape.scm\"",
        "repo=\"https://example.com/p.git\" entrypoint=\"plugin.wasm\"",
        "repo=\"ext::bad\"",
        "repo=42",
        "repo=\"\"",
        "repo=\"file:\"",
        "repo=\"file://remotehost/tools\"",
        "location=\"file:main.scm\" commit=\"abc1234\"",
    ] {
        assert!(
            Config::from_kdl(&format!("plugins {{ panel {declaration}; }}"), None).is_err(),
            "{declaration}"
        );
    }
}

#[test]
fn local_repositories_and_drop_ins_use_the_active_config_directory() {
    let directory = tempfile::tempdir().unwrap();
    assert_ne!(
        directory.path().canonicalize().unwrap(),
        std::env::current_dir().unwrap().canonicalize().unwrap()
    );
    let parsed = Config::from_kdl("plugins { tools repo=\"file:tools\"; }", None).unwrap();
    let RunPluginLocation::Git(source) = &parsed.plugins.aliases["tools"].location else {
        panic!("Git alias")
    };
    assert_eq!(source.repo, "tools");
    let resolved = parsed
        .resolve_plugin_repositories(None, Some(directory.path()))
        .unwrap();
    let RunPluginLocation::Git(source) = &resolved.plugins.aliases["tools"].location else {
        panic!("Git alias")
    };
    assert_eq!(
        std::path::Path::new(&source.repo),
        directory.path().join("tools")
    );
    std::fs::create_dir(directory.path().join("config.d")).unwrap();
    std::fs::write(
        directory.path().join("config.kdl"),
        "plugins { base repo=\"file:base\"; plain repo=\"plain\"; }",
    )
    .unwrap();
    std::fs::write(
        directory.path().join("config.d/tools.kdl"),
        "plugins { tools repo=\"file:tools\"; }",
    )
    .unwrap();
    let config = Config::load(None, Some(directory.path()), false).unwrap();
    for alias in ["base", "plain", "tools"] {
        let RunPluginLocation::Git(source) = &config.plugins.aliases[alias].location else {
            panic!("Git alias")
        };
        assert_eq!(
            std::path::Path::new(&source.repo),
            directory.path().join(alias)
        );
    }
    std::fs::write(
        directory.path().join("config.d/tools.kdl"),
        "plugins { tools repo=\"file:next\"; }",
    )
    .unwrap();
    let reloaded = Config::load(None, Some(directory.path()), false).unwrap();
    let RunPluginLocation::Git(source) = &reloaded.plugins.aliases["tools"].location else {
        panic!("Git alias")
    };
    assert_eq!(
        std::path::Path::new(&source.repo),
        directory.path().join("next")
    );
    std::fs::write(
        directory.path().join("config.d/broken.kdl"),
        "plugins { bad repo=1; }",
    )
    .unwrap();
    assert!(Config::validate_plugin_overlays(None, Some(directory.path())).is_err());
}

#[test]
fn server_assets_resolve_config_and_layout_repository_aliases() {
    use yinz_types::{data::LayoutInfo, input::cli_assets::CliAssets};
    let directory = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    let config_path = files.path().join("config.kdl");
    let layout_path = files.path().join("layout.kdl");
    std::fs::write(
        &config_path,
        "plugins { base repo=\"file:base\"; panel repo=\"old\"; }",
    )
    .unwrap();
    let layout = "plugins { panel repo=\"file:tools\"; }\nlayout { pane; }";
    std::fs::write(&layout_path, layout).unwrap();
    for layout in [
        LayoutInfo::Stringified(layout.into()),
        LayoutInfo::File(
            layout_path.to_string_lossy().into_owned(),
            Default::default(),
        ),
    ] {
        let assets = CliAssets {
            config_file_path: Some(config_path.clone()),
            config_dir: Some(directory.path().to_owned()),
            layout: Some(layout),
            ..Default::default()
        };
        let config = crate::cli_assets::load_config_and_layout(&assets)
            .unwrap()
            .0;
        for (name, repository) in [("base", "base"), ("panel", "tools")] {
            let RunPluginLocation::Git(plugin) = &config.plugins.aliases[name].location else {
                panic!("Git alias")
            };
            assert_eq!(
                std::path::Path::new(&plugin.repo),
                directory.path().join(repository)
            );
        }
        assert_eq!(
            config
                .clone()
                .resolve_plugin_repositories(Some(&config_path), Some(directory.path()))
                .unwrap(),
            config
        );
    }
}
