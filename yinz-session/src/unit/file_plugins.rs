use crate::session_serialization::serialize_session_layout;
use yinz_config::plugins::PluginConfig;
use yinz_types::{
    input::layout::{PluginAlias, Run, RunPlugin, RunPluginLocation, RunPluginOrAlias},
    pane_size::{Dimension, PaneGeom},
    session_manifest::{GlobalLayoutManifest, PaneLayoutManifest, TabLayoutManifest},
};

#[test]
fn file_plugins_survive_saved_layouts_with_directory_fallback() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let name = format!(
        "{} # %20?.scm",
        directory.path().file_name().unwrap().to_str().unwrap()
    );
    let source = "(define (render rows cols) \"\")";
    std::fs::write(directory.path().join(&name), source).unwrap();
    std::fs::write(cwd.path().join("existing.scm"), source).unwrap();
    let file_url = |path| {
        RunPlugin {
            location: RunPluginLocation::File(path),
            ..Default::default()
        }
        .location_for_serialization()
    };
    let mut cases = vec![
        (format!("file:{name}"), Some(directory.path().join(&name))),
        (
            "file:existing.scm".into(),
            Some(cwd.path().join("existing.scm")),
        ),
        (file_url(cwd.path().join(&name)), None),
    ];
    for name in [
        "hash#.scm",
        "percent%20.scm",
        "query?.scm",
        "space name.scm",
    ] {
        let path = cwd.path().join(name);
        std::fs::write(&path, source).unwrap();
        cases.push((file_url(path.clone()), Some(path)));
    }
    #[cfg(unix)]
    {
        let path = cwd.path().join("current # %20?.scm");
        std::os::unix::fs::symlink(cwd.path().join("existing.scm"), &path).unwrap();
        cases.push((file_url(path.clone()), Some(path)));
    }
    for (url, expected_source) in cases {
        let direct = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
            &url,
            &None,
            None,
            Some(cwd.path().to_owned()),
        )
        .unwrap();
        let plugin = direct.get_run_plugin().unwrap();
        let alias = RunPluginOrAlias::Alias(PluginAlias {
            name: "panel".into(),
            run_plugin: Some(plugin.clone()),
            ..Default::default()
        });
        for run in [direct, alias] {
            for floating in [false, true] {
                let pane = PaneLayoutManifest {
                    run: Some(Run::Plugin(run.clone())),
                    geom: PaneGeom {
                        rows: Dimension::fixed(24),
                        cols: Dimension::fixed(80),
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let mut tab = TabLayoutManifest::default();
                if floating {
                    tab.floating_panes.push(pane);
                } else {
                    tab.tiled_panes.push(pane);
                }
                let (raw, _) = serialize_session_layout(GlobalLayoutManifest {
                    tabs: vec![("files".into(), tab)],
                    ..Default::default()
                })
                .unwrap();
                let restored = yinz_config::kdl::layout::from_kdl(&raw, None, None, None).unwrap();
                let (_, tiled, floats) = &restored.tabs[0];
                let restored = if floating {
                    floats[0].run.as_ref()
                } else {
                    tiled.children[0].run.as_ref()
                }
                .unwrap()
                .get_run_plugin()
                .unwrap();
                let resolved = PluginConfig::from_run_plugin(&restored)
                    .unwrap()
                    .resolve_source(Some(directory.path()));
                if let Some(path) = &expected_source {
                    assert_eq!(resolved.unwrap(), (path.clone(), source.to_owned()));
                } else {
                    assert!(resolved.is_err());
                }
                if url.starts_with("file:/") || url == "file:existing.scm" {
                    assert_eq!(restored.location, plugin.location);
                    assert_eq!(restored.relative_path, None);
                    assert_eq!(restored.declared_path, plugin.declared_path);
                } else {
                    assert_eq!(restored.relative_path, plugin.relative_path);
                }
            }
        }
    }
}
