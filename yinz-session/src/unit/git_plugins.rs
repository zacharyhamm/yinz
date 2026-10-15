use crate::session_serialization::serialize_session_layout;
use yinz_types::{
    input::{
        git_plugin::{GitPlugin, GitRevision},
        layout::{Run, RunPlugin, RunPluginLocation, RunPluginOrAlias},
    },
    pane_size::{Dimension, PaneGeom},
    session_manifest::{GlobalLayoutManifest, PaneLayoutManifest, TabLayoutManifest},
};

#[test]
fn git_plugins_survive_saved_layouts() {
    let location = RunPluginLocation::Git(GitPlugin {
        repo: "git@example.com:team/panel.git".into(),
        entrypoint: "src/my panel.scm".into(),
        revision: Some(GitRevision::Tag("v1".into())),
    });
    let run = Run::Plugin(RunPluginOrAlias::RunPlugin(RunPlugin {
        location: location.clone(),
        ..Default::default()
    }));
    let pane = PaneLayoutManifest {
        run: Some(run),
        geom: PaneGeom {
            rows: Dimension::fixed(24),
            cols: Dimension::fixed(80),
            ..Default::default()
        },
        ..Default::default()
    };
    for floating in [false, true] {
        let mut tab = TabLayoutManifest::default();
        if floating {
            tab.floating_panes.push(pane.clone());
        } else {
            tab.tiled_panes.push(pane.clone());
        }
        let (raw, _) = serialize_session_layout(GlobalLayoutManifest {
            tabs: vec![("repository".into(), tab)],
            ..Default::default()
        })
        .unwrap();
        assert!(raw.contains("git://plugin?repo="));
        let restored = yinz_config::kdl::layout::from_kdl(&raw, None, None, None).unwrap();
        let (_, tiled, floats) = &restored.tabs[0];
        let restored = if floating {
            floats[0].run.as_ref()
        } else {
            tiled.children[0].run.as_ref()
        };
        assert_eq!(
            restored.unwrap().get_run_plugin().unwrap().location,
            location
        );
    }
}
