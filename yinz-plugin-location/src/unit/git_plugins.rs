use super::*;
use yinz_types::input::git_plugin::{GitPlugin, GitRevision};

#[test]
fn scheme_repository_entrypoints_preserve_their_extension_and_boundary() {
    for extension in ["scm", "ss", "sld"] {
        let mut plugin = GitPlugin {
            repo: "/tmp/scheme-tools".into(),
            entrypoint: format!("src/main.{extension}").into(),
            revision: None,
        };
        assert_eq!(
            GitPlugin::from_descriptor(&plugin.descriptor()).unwrap(),
            plugin
        );
        plugin.entrypoint = format!("../main.{extension}").into();
        assert!(plugin.validate().is_err());
    }
}

#[test]
fn git_plugin_descriptors_serialize_and_preserve_identity() {
    for repo in [
        "https://example.com/plugins.git?x=1&y=2",
        "git@example.com:team/plugins.git",
        "ssh://git@example.com/plugins.git",
        "/tmp/tools with spaces",
    ] {
        let revision = (!repo.starts_with('/')).then(|| GitRevision::Tag("release/one".into()));
        let plugin = GitPlugin {
            repo: repo.into(),
            entrypoint: "src/my panel.scm".into(),
            revision,
        };
        let location = RunPluginLocation::Git(plugin.clone());
        assert_eq!(
            parse_plugin_location(&location.display(), None).unwrap(),
            location
        );
        assert_eq!(
            serde_json::from_str::<RunPluginLocation>(&serde_json::to_string(&location).unwrap())
                .unwrap(),
            location
        );
        assert_eq!(
            GitPlugin::from_descriptor(&plugin.descriptor()).unwrap(),
            plugin
        );
    }
    for descriptor in [
        "git://plugin?repo=https%3A%2F%2Fexample.com%2Fp.git&branch=main&tag=v1",
        "git://plugin?repo=%2Ftmp%2Ftools&commit=abcdef0",
        "git://plugin?repo=ext%3A%3Abad",
        "git://plugin?repo=a&repo=b",
        "git://wrong?repo=a",
        "git://plugin?repo=a&entrypoint=%2Ftmp%2Fbad.scm",
    ] {
        assert!(
            parse_plugin_location(descriptor, None).is_err(),
            "{descriptor}"
        );
    }
}

#[test]
fn repository_paths_expand_and_saved_layouts_preserve_the_descriptor() {
    let mut plugin = GitPlugin {
        repo: "file:tools".into(),
        entrypoint: "main.scm".into(),
        revision: None,
    };
    resolve_repository_path(&mut plugin, std::path::Path::new("")).unwrap();
    assert_eq!(plugin.repo, "tools");
    resolve_repository_path(&mut plugin, std::path::Path::new("/config")).unwrap();
    assert_eq!(plugin.repo, "/config/tools");
    let mut home = plugin.clone();
    home.repo = "~/src/tools".into();
    resolve_repository_path(&mut home, std::path::Path::new("/config")).unwrap();
    assert!(std::path::Path::new(&home.repo).is_absolute());
    assert!(!home.repo.contains('~'));
}
