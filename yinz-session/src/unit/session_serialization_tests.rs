use super::*;
use expect_test::expect;
use insta::assert_snapshot;
use serde_json::Value;
use std::collections::HashMap;
use yinz_types::{
    input::layout::Layout,
    pane_size::{Dimension, PaneGeom},
};
const PANEGEOMS_JSON: &[&[&str]] = &[
    &[
        r#"{ "x": 0, "y": 1, "rows": { "constraint": "Percent(100.0)", "inner": 43 }, "cols": { "constraint": "Percent(100.0)", "inner": 211 }, "is_stacked": false }"#,
        r#"{ "x": 0, "y": 0, "rows": { "constraint": "Fixed(1)", "inner": 1 }, "cols": { "constraint": "Percent(100.0)", "inner": 211 }, "is_stacked": false }"#,
        r#"{ "x": 0, "y": 44, "rows": { "constraint": "Fixed(2)", "inner": 2 }, "cols": { "constraint": "Percent(100.0)", "inner": 211 }, "is_stacked": false }"#,
    ],
    &[
        r#"{ "x": 0, "y": 0, "rows": { "constraint": "Percent(100.0)", "inner": 26 }, "cols": { "constraint": "Percent(100.0)", "inner": 211 }, "is_stacked": false }"#,
        r#"{ "x": 0, "y": 26, "rows": { "constraint": "Fixed(20)", "inner": 20 }, "cols": { "constraint": "Fixed(50)", "inner": 50 }, "is_stacked": false }"#,
        r#"{ "x": 50, "y": 26, "rows": { "constraint": "Fixed(20)", "inner": 20 }, "cols": { "constraint": "Percent(100.0)", "inner": 161 }, "is_stacked": false }"#,
    ],
    &[
        r#"{ "x": 0, "y": 0, "rows": { "constraint": "Fixed(10)", "inner": 10 }, "cols": { "constraint": "Percent(50.0)", "inner": 106 }, "is_stacked": false }"#,
        r#"{ "x": 106, "y": 0, "rows": { "constraint": "Fixed(10)", "inner": 10 }, "cols": { "constraint": "Percent(50.0)", "inner": 105 }, "is_stacked": false }"#,
        r#"{ "x": 0, "y": 10, "rows": { "constraint": "Percent(100.0)", "inner": 26 }, "cols": { "constraint": "Fixed(40)", "inner": 40 }, "is_stacked": false }"#,
        r#"{ "x": 40, "y": 10, "rows": { "constraint": "Percent(100.0)", "inner": 26 }, "cols": { "constraint": "Percent(100.0)", "inner": 131 }, "is_stacked": false }"#,
        r#"{ "x": 171, "y": 10, "rows": { "constraint": "Percent(100.0)", "inner": 26 }, "cols": { "constraint": "Fixed(40)", "inner": 40 }, "is_stacked": false }"#,
        r#"{ "x": 0, "y": 36, "rows": { "constraint": "Fixed(10)", "inner": 10 }, "cols": { "constraint": "Percent(50.0)", "inner": 106 }, "is_stacked": false }"#,
        r#"{ "x": 106, "y": 36, "rows": { "constraint": "Fixed(10)", "inner": 10 }, "cols": { "constraint": "Percent(50.0)", "inner": 105 }, "is_stacked": false }"#,
    ],
    &[
        r#"{ "x": 0, "y": 0, "rows": { "constraint": "Percent(30.0)", "inner": 11 }, "cols": { "constraint": "Percent(35.0)", "inner": 74 }, "is_stacked": false }"#,
        r#"{ "x": 0, "y": 11, "rows": { "constraint": "Percent(30.0)", "inner": 11 }, "cols": { "constraint": "Percent(35.0)", "inner": 74 }, "is_stacked": false }"#,
        r#"{ "x": 0, "y": 22, "rows": { "constraint": "Percent(40.0)", "inner": 14 }, "cols": { "constraint": "Percent(35.0)", "inner": 74 }, "is_stacked": false }"#,
        r#"{ "x": 74, "y": 0, "rows": { "constraint": "Percent(100.0)", "inner": 36 }, "cols": { "constraint": "Percent(35.0)", "inner": 74 }, "is_stacked": false }"#,
        r#"{ "x": 0, "y": 36, "rows": { "constraint": "Fixed(10)", "inner": 10 }, "cols": { "constraint": "Percent(70.0)", "inner": 148 }, "is_stacked": false }"#,
        r#"{ "x": 148, "y": 0, "rows": { "constraint": "Percent(100.0)", "inner": 46 }, "cols": { "constraint": "Percent(30.0)", "inner": 63 }, "is_stacked": false }"#,
    ],
    &[
        r#"{ "x": 0, "y": 0, "rows": { "constraint": "Fixed(5)", "inner": 5 }, "cols": { "constraint": "Percent(100.0)", "inner": 211 }, "is_stacked": false }"#,
        r#"{ "x": 0, "y": 5, "rows": { "constraint": "Percent(100.0)", "inner": 36 }, "cols": { "constraint": "Fixed(20)", "inner": 20 }, "is_stacked": false }"#,
        r#"{ "x": 20, "y": 5, "rows": { "constraint": "Percent(100.0)", "inner": 36 }, "cols": { "constraint": "Percent(50.0)", "inner": 86 }, "is_stacked": false }"#,
        r#"{ "x": 106, "y": 5, "rows": { "constraint": "Percent(100.0)", "inner": 36 }, "cols": { "constraint": "Percent(50.0)", "inner": 85 }, "is_stacked": false }"#,
        r#"{ "x": 191, "y": 5, "rows": { "constraint": "Percent(100.0)", "inner": 36 }, "cols": { "constraint": "Fixed(20)", "inner": 20 }, "is_stacked": false }"#,
        r#"{ "x": 0, "y": 41, "rows": { "constraint": "Fixed(5)", "inner": 5 }, "cols": { "constraint": "Percent(100.0)", "inner": 211 }, "is_stacked": false }"#,
    ],
];

#[test]
fn editor_line_numbers_survive_tiled_and_floating_session_serialization() {
    for line_number in [Some(42), Some(0), Some(usize::MAX), None] {
        let run = Some(Run::EditFile("/tmp/notes.md".into(), line_number, None));
        let manifest = GlobalLayoutManifest {
            tabs: vec![(
                "editors".to_owned(),
                TabLayoutManifest {
                    tiled_panes: vec![PaneLayoutManifest {
                        run: run.clone(),
                        geom: PaneGeom {
                            rows: Dimension::fixed(20),
                            cols: Dimension::fixed(80),
                            ..Default::default()
                        },
                        ..Default::default()
                    }],
                    floating_panes: vec![PaneLayoutManifest {
                        run: run.clone(),
                        geom: PaneGeom {
                            rows: Dimension::fixed(10),
                            cols: Dimension::fixed(40),
                            ..Default::default()
                        },
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            )],
            ..Default::default()
        };
        let (raw, _) = serialize_session_layout(manifest).unwrap();
        let layout = yinz_config::kdl::layout::from_kdl(&raw, None, None, None).unwrap();
        let (_, tiled, floating) = &layout.tabs()[0];
        let expected = Some(Run::EditFile(
            "/tmp/notes.md".into(),
            line_number.filter(|line| i64::try_from(*line).is_ok()),
            None,
        ));
        assert_eq!(tiled.children[0].run, expected);
        assert_eq!(floating[0].run, expected);
    }
}

#[test]
fn geoms() {
    let geoms = PANEGEOMS_JSON[0]
        .iter()
        .map(|pg| parse_panegeom_from_json(pg))
        .map(|geom| PaneLayoutManifest {
            geom,
            ..Default::default()
        })
        .collect();
    let tab_layout_manifest = TabLayoutManifest {
        tiled_panes: geoms,
        ..Default::default()
    };
    let global_layout_manifest = GlobalLayoutManifest {
        tabs: vec![("Tab #1".to_owned(), tab_layout_manifest)],
        ..Default::default()
    };
    let kdl = serialize_session_layout(global_layout_manifest).unwrap();
    expect![[r#"
            layout {
                tab name="Tab #1" {
                    pane size=1
                    pane
                    pane size=2
                }
            }
        "#]]
    .assert_eq(&kdl.0);

    let geoms = PANEGEOMS_JSON[1]
        .iter()
        .map(|pg| parse_panegeom_from_json(pg))
        .map(|geom| PaneLayoutManifest {
            geom,
            ..Default::default()
        })
        .collect();
    let tab_layout_manifest = TabLayoutManifest {
        tiled_panes: geoms,
        ..Default::default()
    };
    let global_layout_manifest = GlobalLayoutManifest {
        tabs: vec![("Tab #1".to_owned(), tab_layout_manifest)],
        ..Default::default()
    };
    let kdl = serialize_session_layout(global_layout_manifest).unwrap();
    expect![[r#"
            layout {
                tab name="Tab #1" {
                    pane
                    pane size=20 split_direction="vertical" {
                        pane size=50
                        pane
                    }
                }
            }
        "#]]
    .assert_eq(&kdl.0);

    let geoms = PANEGEOMS_JSON[2]
        .iter()
        .map(|pg| parse_panegeom_from_json(pg))
        .map(|geom| PaneLayoutManifest {
            geom,
            ..Default::default()
        })
        .collect();
    let tab_layout_manifest = TabLayoutManifest {
        tiled_panes: geoms,
        ..Default::default()
    };
    let global_layout_manifest = GlobalLayoutManifest {
        tabs: vec![("Tab #1".to_owned(), tab_layout_manifest)],
        ..Default::default()
    };
    let kdl = serialize_session_layout(global_layout_manifest).unwrap();
    expect![[r#"
            layout {
                tab name="Tab #1" {
                    pane size=10 split_direction="vertical" {
                        pane size="50%"
                        pane size="50%"
                    }
                    pane split_direction="vertical" {
                        pane size=40
                        pane
                        pane size=40
                    }
                    pane size=10 split_direction="vertical" {
                        pane size="50%"
                        pane size="50%"
                    }
                }
            }
        "#]]
    .assert_eq(&kdl.0);

    let geoms = PANEGEOMS_JSON[3]
        .iter()
        .map(|pg| parse_panegeom_from_json(pg))
        .map(|geom| PaneLayoutManifest {
            geom,
            ..Default::default()
        })
        .collect();
    let tab_layout_manifest = TabLayoutManifest {
        tiled_panes: geoms,
        ..Default::default()
    };
    let global_layout_manifest = GlobalLayoutManifest {
        tabs: vec![("Tab #1".to_owned(), tab_layout_manifest)],
        ..Default::default()
    };
    let kdl = serialize_session_layout(global_layout_manifest).unwrap();
    expect![[r#"
            layout {
                tab name="Tab #1" {
                    pane split_direction="vertical" {
                        pane size="70%" {
                            pane split_direction="vertical" {
                                pane size="50%" {
                                    pane size="30%"
                                    pane size="30%"
                                    pane size="40%"
                                }
                                pane size="50%"
                            }
                            pane size=10
                        }
                        pane size="30%"
                    }
                }
            }
        "#]]
    .assert_eq(&kdl.0);

    let geoms = PANEGEOMS_JSON[4]
        .iter()
        .map(|pg| parse_panegeom_from_json(pg))
        .map(|geom| PaneLayoutManifest {
            geom,
            ..Default::default()
        })
        .collect();
    let tab_layout_manifest = TabLayoutManifest {
        tiled_panes: geoms,
        ..Default::default()
    };
    let global_layout_manifest = GlobalLayoutManifest {
        tabs: vec![("Tab #1".to_owned(), tab_layout_manifest)],
        ..Default::default()
    };
    let kdl = serialize_session_layout(global_layout_manifest).unwrap();
    expect![[r#"
            layout {
                tab name="Tab #1" {
                    pane size=5
                    pane split_direction="vertical" {
                        pane size=20
                        pane size="50%"
                        pane size="50%"
                        pane size=20
                    }
                    pane size=5
                }
            }
        "#]]
    .assert_eq(&kdl.0);
}

#[test]
fn global_cwd() {
    let global_layout_manifest = GlobalLayoutManifest {
        global_cwd: Some(PathBuf::from("/path/to/m\"y/global cwd")),
        ..Default::default()
    };
    let kdl = serialize_session_layout(global_layout_manifest).unwrap();
    assert_snapshot!(kdl.0);
}

#[test]
fn can_serialize_tab_name() {
    let global_layout_manifest = GlobalLayoutManifest {
        tabs: vec![("my \"tab \\name".to_owned(), TabLayoutManifest::default())],
        ..Default::default()
    };
    let kdl = serialize_session_layout(global_layout_manifest).unwrap();
    assert_snapshot!(kdl.0);
}
#[test]
fn can_serialize_tab_focus() {
    let tab_layout_manifest = TabLayoutManifest {
        is_focused: true,
        ..Default::default()
    };
    let global_layout_manifest = GlobalLayoutManifest {
        tabs: vec![("Tab #1".to_owned(), tab_layout_manifest)],
        ..Default::default()
    };
    let kdl = serialize_session_layout(global_layout_manifest).unwrap();
    assert_snapshot!(kdl.0);
}
#[test]
fn can_serialize_tab_hide_floating_panes() {
    let tab_layout_manifest = TabLayoutManifest {
        hide_floating_panes: true,
        ..Default::default()
    };
    let global_layout_manifest = GlobalLayoutManifest {
        tabs: vec![("Tab #1".to_owned(), tab_layout_manifest)],
        ..Default::default()
    };
    let kdl = serialize_session_layout(global_layout_manifest).unwrap();
    assert_snapshot!(kdl.0);
}
#[test]
fn can_serialize_tab_user_named() {
    let tab_layout_manifest = TabLayoutManifest {
        user_named: true,
        ..Default::default()
    };
    let global_layout_manifest = GlobalLayoutManifest {
        tabs: vec![("notes".to_owned(), tab_layout_manifest)],
        ..Default::default()
    };
    let kdl = serialize_session_layout(global_layout_manifest).unwrap();
    assert_snapshot!(kdl.0);
}
#[test]
fn can_serialize_tab_with_tiled_panes() {
    use yinz_types::input::command::RunCommand;

    let mut plugin_configuration = BTreeMap::new();
    plugin_configuration.insert("key 1\"\\".to_owned(), "val 1\"\\".to_owned());
    plugin_configuration.insert("key 2\"\\".to_owned(), "val 2\"\\".to_owned());
    let tab_layout_manifest = TabLayoutManifest {
        tiled_panes: vec![
            PaneLayoutManifest {
                geom: PaneGeom {
                    x: 0,
                    y: 0,
                    rows: Dimension::fixed(10),
                    cols: Dimension::fixed(10),
                    stacked: None,
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
            PaneLayoutManifest {
                run: Some(Run::Cwd(PathBuf::from("/tmp/\"my/cool cwd"))),
                geom: PaneGeom {
                    x: 0,
                    y: 10,
                    rows: Dimension::fixed(10),
                    cols: Dimension::fixed(10),
                    stacked: None,
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
            PaneLayoutManifest {
                run: Some(Run::EditFile(
                    PathBuf::from("/tmp/\"my/cool cwd/my-file"),
                    None,
                    None,
                )),
                geom: PaneGeom {
                    x: 0,
                    y: 20,
                    rows: Dimension::fixed(10),
                    cols: Dimension::fixed(10),
                    stacked: None,
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
            PaneLayoutManifest {
                run: Some(Run::Command(RunCommand {
                    command: PathBuf::from("/tmp/\"my/cool cwd/command.sh"),
                    ..Default::default()
                })),
                geom: PaneGeom {
                    x: 0,
                    y: 30,
                    rows: Dimension::fixed(10),
                    cols: Dimension::fixed(10),
                    stacked: None,
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
            PaneLayoutManifest {
                run: Some(Run::Command(RunCommand {
                    command: PathBuf::from("/tmp/\"my/cool cwd/command.sh"),
                    args: vec![
                        "--arg1".to_owned(),
                        "arg\"2".to_owned(),
                        "arg > \\3".to_owned(),
                    ],
                    ..Default::default()
                })),
                geom: PaneGeom {
                    x: 0,
                    y: 40,
                    rows: Dimension::fixed(10),
                    cols: Dimension::fixed(10),
                    stacked: None,
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
            PaneLayoutManifest {
                run: Some(Run::Plugin(RunPluginOrAlias::RunPlugin(
                    yinz_runtime::plugin_location::run_plugin_from_url(
                        "file:/tmp/\"my/cool cwd/plugin.scm",
                    )
                    .unwrap(),
                ))),
                geom: PaneGeom {
                    x: 0,
                    y: 50,
                    rows: Dimension::fixed(10),
                    cols: Dimension::fixed(10),
                    stacked: None,
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
            PaneLayoutManifest {
                run: Some(Run::Plugin(RunPluginOrAlias::RunPlugin(
                    yinz_runtime::plugin_location::run_plugin_from_url(
                        "file:/tmp/\"my/cool cwd/plugin.scm",
                    )
                    .unwrap()
                    .with_configuration(plugin_configuration),
                ))),
                geom: PaneGeom {
                    x: 0,
                    y: 60,
                    rows: Dimension::fixed(10),
                    cols: Dimension::fixed(10),
                    stacked: None,
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
            PaneLayoutManifest {
                is_borderless: true,
                geom: PaneGeom {
                    x: 0,
                    y: 70,
                    rows: Dimension::fixed(10),
                    cols: Dimension::fixed(10),
                    stacked: None,
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
            PaneLayoutManifest {
                title: Some("my cool \\ \"pane_title\"".to_owned()),
                is_focused: true,
                pane_contents: Some("can has pane contents".to_owned()),
                geom: PaneGeom {
                    x: 0,
                    y: 80,
                    rows: Dimension::fixed(10),
                    cols: Dimension::fixed(10),
                    stacked: None,
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let global_layout_manifest = GlobalLayoutManifest {
        tabs: vec![("Tab with \"tiled panes\"".to_owned(), tab_layout_manifest)],
        ..Default::default()
    };
    let kdl = serialize_session_layout(global_layout_manifest).unwrap();
    assert_snapshot!(kdl.0);
}
#[test]
fn can_serialize_tab_with_floating_panes() {
    use yinz_types::input::command::RunCommand;

    let mut plugin_configuration = BTreeMap::new();
    plugin_configuration.insert("key 1\"\\".to_owned(), "val 1\"\\".to_owned());
    plugin_configuration.insert("key 2\"\\".to_owned(), "val 2\"\\".to_owned());
    let tab_layout_manifest = TabLayoutManifest {
        floating_panes: vec![
            PaneLayoutManifest {
                geom: PaneGeom {
                    x: 0,
                    y: 0,
                    rows: Dimension::fixed(10),
                    cols: Dimension::fixed(10),
                    stacked: None,
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
            PaneLayoutManifest {
                run: Some(Run::Cwd(PathBuf::from("/tmp/\"my/cool cwd"))),
                geom: PaneGeom {
                    x: 0,
                    y: 10,
                    rows: Dimension::fixed(10),
                    cols: Dimension::fixed(10),
                    stacked: None,
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
            PaneLayoutManifest {
                run: Some(Run::EditFile(
                    PathBuf::from("/tmp/\"my/cool cwd/my-file"),
                    None,
                    None,
                )),
                geom: PaneGeom {
                    x: 0,
                    y: 20,
                    rows: Dimension::fixed(10),
                    cols: Dimension::fixed(10),
                    stacked: None,
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
            PaneLayoutManifest {
                run: Some(Run::Command(RunCommand {
                    command: PathBuf::from("/tmp/\"my/cool cwd/command.sh"),
                    ..Default::default()
                })),
                geom: PaneGeom {
                    x: 0,
                    y: 30,
                    rows: Dimension::fixed(10),
                    cols: Dimension::fixed(10),
                    stacked: None,
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
            PaneLayoutManifest {
                run: Some(Run::Command(RunCommand {
                    command: PathBuf::from("/tmp/\"my/cool cwd/command.sh"),
                    args: vec![
                        "--arg1".to_owned(),
                        "arg\"2".to_owned(),
                        "arg > \\3".to_owned(),
                    ],
                    ..Default::default()
                })),
                geom: PaneGeom {
                    x: 0,
                    y: 40,
                    rows: Dimension::fixed(10),
                    cols: Dimension::fixed(10),
                    stacked: None,
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
            PaneLayoutManifest {
                run: Some(Run::Plugin(RunPluginOrAlias::RunPlugin(
                    yinz_runtime::plugin_location::run_plugin_from_url(
                        "file:/tmp/\"my/cool cwd/plugin.scm",
                    )
                    .unwrap(),
                ))),
                geom: PaneGeom {
                    x: 0,
                    y: 50,
                    rows: Dimension::fixed(10),
                    cols: Dimension::fixed(10),
                    stacked: None,
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
            PaneLayoutManifest {
                run: Some(Run::Plugin(RunPluginOrAlias::RunPlugin(
                    yinz_runtime::plugin_location::run_plugin_from_url(
                        "file:/tmp/\"my/cool cwd/plugin.scm",
                    )
                    .unwrap()
                    .with_configuration(plugin_configuration),
                ))),
                geom: PaneGeom {
                    x: 0,
                    y: 60,
                    rows: Dimension::fixed(10),
                    cols: Dimension::fixed(10),
                    stacked: None,
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
            PaneLayoutManifest {
                // note that in this case, `is_borderless` should be ignored because this is a
                // floating pane
                is_borderless: true,
                geom: PaneGeom {
                    x: 0,
                    y: 70,
                    rows: Dimension::fixed(10),
                    cols: Dimension::fixed(10),
                    stacked: None,
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
            PaneLayoutManifest {
                title: Some("my cool \\ \"pane_title\"".to_owned()),
                is_focused: true,
                pane_contents: Some("can has pane contents".to_owned()),
                geom: PaneGeom {
                    x: 0,
                    y: 80,
                    rows: Dimension::fixed(10),
                    cols: Dimension::fixed(10),
                    stacked: None,
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let global_layout_manifest = GlobalLayoutManifest {
        tabs: vec![(
            "Tab with \"floating panes\"".to_owned(),
            tab_layout_manifest,
        )],
        ..Default::default()
    };
    let kdl = serialize_session_layout(global_layout_manifest).unwrap();
    assert_snapshot!(kdl.0);
}
#[test]
fn can_serialize_tab_with_stacked_panes() {
    let tab_layout_manifest = TabLayoutManifest {
        tiled_panes: vec![
            PaneLayoutManifest {
                geom: PaneGeom {
                    x: 0,
                    y: 0,
                    rows: Dimension::fixed(1),
                    cols: Dimension::fixed(10),
                    stacked: Some(0),
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
            PaneLayoutManifest {
                geom: PaneGeom {
                    x: 0,
                    y: 1,
                    rows: Dimension::fixed(10),
                    cols: Dimension::fixed(10),
                    stacked: Some(0),
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
            PaneLayoutManifest {
                geom: PaneGeom {
                    x: 0,
                    y: 11,
                    rows: Dimension::fixed(1),
                    cols: Dimension::fixed(10),
                    stacked: Some(0),
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let global_layout_manifest = GlobalLayoutManifest {
        tabs: vec![("Tab with \"stacked panes\"".to_owned(), tab_layout_manifest)],
        ..Default::default()
    };
    let kdl = serialize_session_layout(global_layout_manifest).unwrap();
    assert_snapshot!(kdl.0);
}
#[test]
fn can_serialize_tab_with_multiple_stacked_panes_in_the_same_node() {
    let tab_layout_manifest = TabLayoutManifest {
        tiled_panes: vec![
            PaneLayoutManifest {
                geom: PaneGeom {
                    x: 0,
                    y: 0,
                    rows: Dimension::fixed(1),
                    cols: Dimension::fixed(10),
                    stacked: Some(0),
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
            PaneLayoutManifest {
                geom: PaneGeom {
                    x: 0,
                    y: 1,
                    rows: Dimension::fixed(10),
                    cols: Dimension::fixed(10),
                    stacked: Some(0),
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
            PaneLayoutManifest {
                geom: PaneGeom {
                    x: 0,
                    y: 11,
                    rows: Dimension::fixed(1),
                    cols: Dimension::fixed(10),
                    stacked: Some(0),
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
            PaneLayoutManifest {
                geom: PaneGeom {
                    x: 0,
                    y: 12,
                    rows: Dimension::fixed(10),
                    cols: Dimension::fixed(10),
                    stacked: None,
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
            PaneLayoutManifest {
                geom: PaneGeom {
                    x: 0,
                    y: 22,
                    rows: Dimension::fixed(1),
                    cols: Dimension::fixed(10),
                    stacked: Some(1),
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
            PaneLayoutManifest {
                geom: PaneGeom {
                    x: 0,
                    y: 23,
                    rows: Dimension::fixed(10),
                    cols: Dimension::fixed(10),
                    stacked: Some(1),
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
            PaneLayoutManifest {
                geom: PaneGeom {
                    x: 0,
                    y: 33,
                    rows: Dimension::fixed(1),
                    cols: Dimension::fixed(10),
                    stacked: Some(1),
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let global_layout_manifest = GlobalLayoutManifest {
        tabs: vec![("Tab with \"stacked panes\"".to_owned(), tab_layout_manifest)],
        ..Default::default()
    };
    let kdl = serialize_session_layout(global_layout_manifest).unwrap();
    assert_snapshot!(kdl.0);
}
#[test]
fn can_serialize_tab_with_multiple_stacks_next_to_eachother() {
    let tab_layout_manifest = TabLayoutManifest {
        tiled_panes: vec![
            PaneLayoutManifest {
                geom: PaneGeom {
                    x: 0,
                    y: 0,
                    rows: Dimension::fixed(1),
                    cols: Dimension::fixed(10),
                    stacked: Some(0),
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
            PaneLayoutManifest {
                geom: PaneGeom {
                    x: 0,
                    y: 1,
                    rows: Dimension::fixed(10),
                    cols: Dimension::fixed(10),
                    stacked: Some(0),
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
            PaneLayoutManifest {
                geom: PaneGeom {
                    x: 0,
                    y: 11,
                    rows: Dimension::fixed(1),
                    cols: Dimension::fixed(10),
                    stacked: Some(0),
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
            PaneLayoutManifest {
                geom: PaneGeom {
                    x: 0,
                    y: 12,
                    rows: Dimension::fixed(10),
                    cols: Dimension::fixed(10),
                    stacked: None,
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
            PaneLayoutManifest {
                geom: PaneGeom {
                    x: 0,
                    y: 22,
                    rows: Dimension::fixed(1),
                    cols: Dimension::fixed(10),
                    stacked: Some(1),
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
            PaneLayoutManifest {
                geom: PaneGeom {
                    x: 0,
                    y: 23,
                    rows: Dimension::fixed(10),
                    cols: Dimension::fixed(10),
                    stacked: Some(1),
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
            PaneLayoutManifest {
                geom: PaneGeom {
                    x: 0,
                    y: 33,
                    rows: Dimension::fixed(1),
                    cols: Dimension::fixed(10),
                    stacked: Some(1),
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
            PaneLayoutManifest {
                geom: PaneGeom {
                    x: 10,
                    y: 0,
                    rows: Dimension::fixed(1),
                    cols: Dimension::fixed(10),
                    stacked: Some(2),
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
            PaneLayoutManifest {
                geom: PaneGeom {
                    x: 10,
                    y: 1,
                    rows: Dimension::fixed(10),
                    cols: Dimension::fixed(10),
                    stacked: Some(2),
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
            PaneLayoutManifest {
                geom: PaneGeom {
                    x: 10,
                    y: 11,
                    rows: Dimension::fixed(1),
                    cols: Dimension::fixed(10),
                    stacked: Some(2),
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
            PaneLayoutManifest {
                geom: PaneGeom {
                    x: 10,
                    y: 12,
                    rows: Dimension::fixed(10),
                    cols: Dimension::fixed(10),
                    stacked: None,
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
            PaneLayoutManifest {
                geom: PaneGeom {
                    x: 10,
                    y: 22,
                    rows: Dimension::fixed(1),
                    cols: Dimension::fixed(10),
                    stacked: Some(3),
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
            PaneLayoutManifest {
                geom: PaneGeom {
                    x: 10,
                    y: 23,
                    rows: Dimension::fixed(10),
                    cols: Dimension::fixed(10),
                    stacked: Some(3),
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
            PaneLayoutManifest {
                geom: PaneGeom {
                    x: 10,
                    y: 33,
                    rows: Dimension::fixed(1),
                    cols: Dimension::fixed(10),
                    stacked: Some(3),
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let global_layout_manifest = GlobalLayoutManifest {
        tabs: vec![("Tab with \"stacked panes\"".to_owned(), tab_layout_manifest)],
        ..Default::default()
    };
    let kdl = serialize_session_layout(global_layout_manifest).unwrap();
    assert_snapshot!(kdl.0);
}
#[test]
fn can_serialize_multiple_tabs() {
    let tab_1_layout_manifest = TabLayoutManifest {
        tiled_panes: vec![PaneLayoutManifest {
            geom: PaneGeom {
                x: 0,
                y: 0,
                rows: Dimension::percent(100.0),
                cols: Dimension::percent(100.0),
                stacked: None,
                is_pinned: false,
                logical_position: None,
            },
            ..Default::default()
        }],
        ..Default::default()
    };
    let tab_2_layout_manifest = TabLayoutManifest {
        tiled_panes: vec![
            PaneLayoutManifest {
                geom: PaneGeom {
                    x: 0,
                    y: 0,
                    rows: Dimension::fixed(10),
                    cols: Dimension::fixed(10),
                    stacked: None,
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
            PaneLayoutManifest {
                geom: PaneGeom {
                    x: 10,
                    y: 0,
                    rows: Dimension::fixed(10),
                    cols: Dimension::fixed(10),
                    stacked: None,
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let global_layout_manifest = GlobalLayoutManifest {
        tabs: vec![
            ("First tab".to_owned(), tab_1_layout_manifest),
            ("Second tab".to_owned(), tab_2_layout_manifest),
        ],
        ..Default::default()
    };
    let kdl = serialize_session_layout(global_layout_manifest).unwrap();
    assert_snapshot!(kdl.0);
}
#[test]
fn pane_tabs_serialize_descriptors_configuration_and_selected_index() {
    use yinz_types::pane_tabs::*;
    let legacy: PaneTabDescriptor =
        serde_json::from_str(r#"{"label":"Files","location":"yinz:strider","configuration":{}}"#)
            .unwrap();
    assert_eq!(legacy.initial_cwd, None);
    let tabs = PaneTabsLayout {
        active: 1,
        tabs: vec![
            PaneTabDescriptor {
                label: "Files".into(),
                location: "yinz:strider".into(),
                initial_cwd: Some(PathBuf::from("/tmp/files")),
                configuration: BTreeMap::new(),
            },
            PaneTabDescriptor {
                label: "Files!".into(),
                location: "yinz:strider".into(),
                initial_cwd: None,
                configuration: BTreeMap::from([("custom".into(), "value".into())]),
            },
        ],
    };
    let pane = PaneLayoutManifest {
        pane_tabs: Some(tabs.clone()),
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
            tabs: vec![("tabs".into(), tab)],
            ..Default::default()
        })
        .unwrap();
        let restored = yinz_config::kdl::layout::from_kdl(&raw, None, None, None).unwrap();
        let (_, tiled, floats) = &restored.tabs[0];
        let actual = if floating {
            floats[0].pane_tabs.as_ref()
        } else {
            tiled.children[0].pane_tabs.as_ref()
        };
        let expected = tabs.clone();
        assert_eq!(actual, Some(&expected), "{raw}");
    }
}

#[test]
fn can_serialize_tab_with_a_single_tiled_pane() {
    use yinz_types::input::command::RunCommand;
    let tab_layout_manifest = TabLayoutManifest {
        tiled_panes: vec![PaneLayoutManifest {
            run: Some(Run::Command(RunCommand {
                command: PathBuf::from("/bin/sleep"),
                args: vec!["10000".to_owned()],
                ..Default::default()
            })),
            title: Some("my-only-pane".to_owned()),
            geom: PaneGeom {
                x: 0,
                y: 0,
                rows: Dimension::fixed(10),
                cols: Dimension::fixed(10),
                stacked: None,
                is_pinned: false,
                logical_position: None,
            },
            ..Default::default()
        }],
        ..Default::default()
    };
    let global_layout_manifest = GlobalLayoutManifest {
        tabs: vec![("Tab #1".to_owned(), tab_layout_manifest)],
        ..Default::default()
    };
    let kdl = serialize_session_layout(global_layout_manifest).unwrap();
    expect![[r#"
            layout {
                tab name="Tab #1" {
                    pane command="/bin/sleep" name="my-only-pane" {
                        args "10000"
                        start_suspended true
                    }
                }
            }
        "#]]
    .assert_eq(&kdl.0);
}
#[test]
fn can_serialize_tab_with_a_single_tiled_pane_and_a_floating_pane() {
    let tab_layout_manifest = TabLayoutManifest {
        tiled_panes: vec![PaneLayoutManifest {
            title: Some("my-only-tiled-pane".to_owned()),
            geom: PaneGeom {
                x: 0,
                y: 0,
                rows: Dimension::fixed(10),
                cols: Dimension::fixed(10),
                stacked: None,
                is_pinned: false,
                logical_position: None,
            },
            ..Default::default()
        }],
        floating_panes: vec![PaneLayoutManifest {
            title: Some("my-floating-pane".to_owned()),
            geom: PaneGeom {
                x: 1,
                y: 1,
                rows: Dimension::fixed(5),
                cols: Dimension::fixed(5),
                stacked: None,
                is_pinned: false,
                logical_position: None,
            },
            ..Default::default()
        }],
        ..Default::default()
    };
    let global_layout_manifest = GlobalLayoutManifest {
        tabs: vec![("Tab #1".to_owned(), tab_layout_manifest)],
        ..Default::default()
    };
    let kdl = serialize_session_layout(global_layout_manifest).unwrap();
    expect![[r#"
            layout {
                tab name="Tab #1" {
                    pane name="my-only-tiled-pane"
                    floating_panes {
                        pane name="my-floating-pane" {
                            height 5
                            width 5
                            x 1
                            y 1
                        }
                    }
                }
            }
        "#]]
    .assert_eq(&kdl.0);
}

/// A sandboxed pane resurrects into its sandbox: its command is the agent's own, so
/// without the attribute it would come back running on the host.
#[test]
fn can_serialize_bubblewrap_panes() {
    use yinz_types::input::command::RunCommand;
    let sandboxed = RunCommand {
        command: PathBuf::from("claude"),
        args: vec!["--resume".to_owned()],
        cwd: Some(PathBuf::from("/repo/.worktrees/task")),
        hold_on_close: true,
        target: PaneTarget::Bubblewrap {
            root: PathBuf::from("/repo"),
        },
        ..Default::default()
    };
    let tab_layout_manifest = TabLayoutManifest {
        tiled_panes: vec![PaneLayoutManifest {
            run: Some(Run::Command(sandboxed.clone())),
            geom: PaneGeom {
                x: 0,
                y: 0,
                rows: Dimension::fixed(10),
                cols: Dimension::fixed(10),
                stacked: None,
                is_pinned: false,
                logical_position: None,
            },
            ..Default::default()
        }],
        ..Default::default()
    };
    let global_layout_manifest = GlobalLayoutManifest {
        tabs: vec![("Tab #1".to_owned(), tab_layout_manifest)],
        ..Default::default()
    };
    let kdl = serialize_session_layout(global_layout_manifest).unwrap();
    assert!(kdl.0.contains(r#"bubblewrap="/repo""#), "{}", kdl.0);

    let layout =
        yinz_config::kdl::layout::from_kdl(&kdl.0, Some("dump.kdl".into()), None, None).unwrap();
    let (_, tiled, _) = &layout.tabs[0];
    let mut panes = vec![tiled];
    let mut resurrected = None;
    while let Some(pane) = panes.pop() {
        if let Some(Run::Command(command)) = &pane.run {
            resurrected = Some(command.clone());
        }
        panes.extend(&pane.children);
    }
    assert_eq!(
        resurrected,
        Some(RunCommand {
            hold_on_start: true,
            ..sandboxed
        })
    );
}
#[test]
fn can_serialize_new_tab_template_with_a_single_pane() {
    let tiled_panes_layout = TiledPaneLayout {
        name: Some("my-only-template-pane".to_owned()),
        ..Default::default()
    };
    let default_layout = Layout {
        template: Some((tiled_panes_layout, vec![])),
        ..Default::default()
    };
    let global_layout_manifest = GlobalLayoutManifest {
        default_layout: Box::new(default_layout),
        ..Default::default()
    };
    let kdl = serialize_session_layout(global_layout_manifest).unwrap();
    expect![[r#"
            layout {
                new_tab_template {
                    pane name="my-only-template-pane"
                }
            }
        "#]]
    .assert_eq(&kdl.0);
}
#[test]
fn can_serialize_swap_tiled_layout_with_a_single_pane() {
    let tiled_panes_layout = TiledPaneLayout {
        name: Some("my-only-swap-pane".to_owned()),
        ..Default::default()
    };
    let mut swap_tiled_layout = BTreeMap::new();
    swap_tiled_layout.insert(LayoutConstraint::NoConstraint, tiled_panes_layout);
    let default_layout = Layout {
        swap_tiled_layouts: vec![(swap_tiled_layout, Some("my-swap".to_owned()))],
        ..Default::default()
    };
    let global_layout_manifest = GlobalLayoutManifest {
        default_layout: Box::new(default_layout),
        ..Default::default()
    };
    let kdl = serialize_session_layout(global_layout_manifest).unwrap();
    expect![[r#"
            layout {
                swap_tiled_layout name="my-swap" {
                    tab {
                        pane name="my-only-swap-pane"
                    }
                }
            }
        "#]]
    .assert_eq(&kdl.0);
}
#[test]
fn can_serialize_new_tab_template() {
    let tiled_panes_layout = TiledPaneLayout {
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };

    let floating_panes_layout = vec![
        FloatingPaneLayout::default(),
        FloatingPaneLayout::default(),
        FloatingPaneLayout::default(),
    ];
    let default_layout = Box::new(Layout {
        template: Some((tiled_panes_layout, floating_panes_layout)),
        ..Default::default()
    });
    let global_layout_manifest = GlobalLayoutManifest {
        default_layout,
        ..Default::default()
    };
    let kdl = serialize_session_layout(global_layout_manifest).unwrap();
    assert_snapshot!(kdl.0);
}
#[test]
fn can_serialize_swap_tiled_panes() {
    let tiled_panes_layout = TiledPaneLayout {
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut default_layout = Layout::default();
    let mut swap_tiled_layout_1 = BTreeMap::new();
    let mut swap_tiled_layout_2 = BTreeMap::new();
    swap_tiled_layout_1.insert(LayoutConstraint::MaxPanes(1), tiled_panes_layout.clone());
    swap_tiled_layout_1.insert(LayoutConstraint::MinPanes(1), tiled_panes_layout.clone());
    swap_tiled_layout_1.insert(LayoutConstraint::ExactPanes(1), tiled_panes_layout.clone());
    swap_tiled_layout_1.insert(LayoutConstraint::NoConstraint, tiled_panes_layout.clone());
    swap_tiled_layout_2.insert(LayoutConstraint::MaxPanes(2), tiled_panes_layout.clone());
    swap_tiled_layout_2.insert(LayoutConstraint::MinPanes(2), tiled_panes_layout.clone());
    swap_tiled_layout_2.insert(LayoutConstraint::ExactPanes(2), tiled_panes_layout.clone());
    swap_tiled_layout_2.insert(LayoutConstraint::NoConstraint, tiled_panes_layout.clone());

    let swap_tiled_layouts = vec![
        (swap_tiled_layout_1, None),
        (swap_tiled_layout_2, Some("swap_tiled_layout_2".to_owned())),
    ];
    default_layout.swap_tiled_layouts = swap_tiled_layouts;
    let default_layout = Box::new(default_layout);
    let global_layout_manifest = GlobalLayoutManifest {
        default_layout,
        ..Default::default()
    };
    let kdl = serialize_session_layout(global_layout_manifest).unwrap();
    assert_snapshot!(kdl.0);
}
#[test]
fn can_serialize_swap_floating_panes() {
    let floating_panes_layout = vec![
        FloatingPaneLayout::default(),
        FloatingPaneLayout::default(),
        FloatingPaneLayout::default(),
    ];
    let mut default_layout = Layout::default();
    let mut swap_floating_layout_1 = BTreeMap::new();
    let mut swap_floating_layout_2 = BTreeMap::new();
    swap_floating_layout_1.insert(LayoutConstraint::MaxPanes(1), floating_panes_layout.clone());
    swap_floating_layout_1.insert(LayoutConstraint::MinPanes(1), floating_panes_layout.clone());
    swap_floating_layout_1.insert(
        LayoutConstraint::ExactPanes(1),
        floating_panes_layout.clone(),
    );
    swap_floating_layout_1.insert(
        LayoutConstraint::NoConstraint,
        floating_panes_layout.clone(),
    );
    swap_floating_layout_2.insert(LayoutConstraint::MaxPanes(2), floating_panes_layout.clone());
    swap_floating_layout_2.insert(LayoutConstraint::MinPanes(2), floating_panes_layout.clone());
    swap_floating_layout_2.insert(
        LayoutConstraint::ExactPanes(2),
        floating_panes_layout.clone(),
    );
    swap_floating_layout_2.insert(
        LayoutConstraint::NoConstraint,
        floating_panes_layout.clone(),
    );

    let swap_floating_layouts = vec![
        (swap_floating_layout_1, None),
        (
            swap_floating_layout_2,
            Some("swap_floating_layout_2".to_owned()),
        ),
    ];
    default_layout.swap_floating_layouts = swap_floating_layouts;
    let default_layout = Box::new(default_layout);
    let global_layout_manifest = GlobalLayoutManifest {
        default_layout,
        ..Default::default()
    };
    let kdl = serialize_session_layout(global_layout_manifest).unwrap();
    assert_snapshot!(kdl.0);
}

// ---------------------------------------------------------------------
// Arrangements that are not a tree of splits
// ---------------------------------------------------------------------

fn manifest_at(x: usize, y: usize, cols: usize, rows: usize) -> PaneLayoutManifest {
    // `cols`/`rows` are `Dimension`s and are set through `set_inner` rather than assigned, so the
    // struct literal covers x and y and the two dimensions follow.
    let mut geom = PaneGeom {
        x,
        y,
        ..Default::default()
    };
    geom.cols.set_inner(cols);
    geom.rows.set_inner(rows);
    PaneLayoutManifest {
        geom,
        ..Default::default()
    }
}

fn stacked(mut manifest: PaneLayoutManifest, stack_id: usize) -> PaneLayoutManifest {
    manifest.geom.stacked = Some(stack_id);
    manifest
}

/// The ui strips span the screen, the panes between them only part of it. No
/// split line spans this in either direction, and none of it is stacked.
fn a_tab_that_is_not_a_rectangle() -> Vec<PaneLayoutManifest> {
    vec![
        manifest_at(0, 0, 100, 1),
        manifest_at(20, 1, 40, 19),
        manifest_at(60, 1, 40, 19),
        manifest_at(20, 20, 40, 19),
        manifest_at(60, 20, 40, 19),
        manifest_at(0, 39, 100, 1),
    ]
}

#[test]
fn an_arrangement_that_is_not_a_layout_is_refused_rather_than_shrunk() {
    // It used to come back as a single empty pane standing in for all six -
    // the shape of bug that loses five panes without saying anything.
    let arrangement = a_tab_that_is_not_a_rectangle();
    assert!(get_tiled_panes_layout_from_panegeoms(&arrangement, None).is_none());
}

#[test]
fn a_stack_layout_is_refused_when_something_in_it_is_not_stacked() {
    // `stack_layout_from_manifest` is only reached on the assumption that
    // everything left is in a stack. When that is wrong, the panes that are
    // not have nowhere to go, and saying so beats leaving them out.
    let mixed = vec![
        stacked(manifest_at(0, 0, 50, 1), 1),
        stacked(manifest_at(0, 1, 50, 19), 1),
        manifest_at(50, 0, 50, 20),
    ];
    assert!(stack_layout_from_manifest(&mixed, None).is_none());
    assert!(stack_layout_from_manifest(&vec![manifest_at(0, 0, 50, 20)], None).is_none());
}

#[test]
fn an_approximated_arrangement_keeps_every_pane() {
    let arrangement = a_tab_that_is_not_a_rectangle();
    let layout = tiled_layout_from_panegeoms_lossy(&arrangement);
    assert_eq!(layout.pane_count(), arrangement.len());
}

#[test]
fn an_approximation_keeps_a_stack_together() {
    // A stack cut in half arrives as two stacks, so the cuts go between them.
    let arrangement = vec![
        manifest_at(0, 0, 100, 1),
        stacked(manifest_at(20, 1, 80, 1), 1),
        stacked(manifest_at(20, 2, 80, 1), 1),
        stacked(manifest_at(20, 3, 80, 36), 1),
    ];
    let layout = tiled_layout_from_panegeoms_lossy(&arrangement);
    assert_eq!(layout.pane_count(), 4);

    fn stack_sizes(layout: &TiledPaneLayout, sizes: &mut Vec<usize>) {
        if layout.children_are_stacked {
            sizes.push(layout.children.len());
        }
        for child in &layout.children {
            stack_sizes(child, sizes);
        }
    }
    let mut sizes = vec![];
    stack_sizes(&layout, &mut sizes);
    assert_eq!(
        sizes,
        vec![3],
        "the stack should arrive as one stack of three"
    );
}

#[test]
fn an_approximation_preserves_positions_after_rounding() {
    // The approximation is only reached for arrangements no layout describes,
    // but it should not be a *different* arrangement when handed one that a
    // layout does describe - a cut nothing straddles is a real split. Checked
    // against the arrangements these tests already have: each pane must come
    // back nearer its own rectangle than any other pane's, which is the
    // strongest thing that can be asked of something allowed to round.
    for panegeoms in PANEGEOMS_JSON.iter() {
        // each pane named after its index, so the leaves can be paired with
        // the panes they came from rather than with wherever they turn up
        let arrangement: Vec<PaneLayoutManifest> = panegeoms
            .iter()
            .map(|pg| parse_panegeom_from_json(pg))
            .enumerate()
            .map(|(index, geom)| PaneLayoutManifest {
                geom,
                run: Some(Run::Cwd(PathBuf::from(index.to_string()))),
                ..Default::default()
            })
            .collect();
        let approximated = approximate_tiled_layout(&arrangement, None);
        assert_eq!(approximated.pane_count(), arrangement.len());

        let mut space = PaneGeom::default();
        space.cols.set_inner(
            arrangement
                .iter()
                .map(|pane| pane.geom.x + pane.geom.cols.as_usize())
                .max()
                .unwrap(),
        );
        space.rows.set_inner(
            arrangement
                .iter()
                .map(|pane| pane.geom.y + pane.geom.rows.as_usize())
                .max()
                .unwrap(),
        );
        let positioned = approximated.position_panes_in_space(&space, None, false, false);
        let positioned = positioned.unwrap();
        let landed: Vec<(usize, PaneGeom)> = positioned
            .into_iter()
            .map(|(layout, geom)| match layout.run {
                Some(Run::Cwd(name)) => (name.to_str().unwrap().parse::<usize>().unwrap(), geom),
                other => panic!("a pane came back without its name: {other:?}"),
            })
            .collect();
        assert_eq!(landed.len(), arrangement.len());
        let moved_by =
            |from: &PaneGeom, to: &PaneGeom| from.x.abs_diff(to.x) + from.y.abs_diff(to.y);
        for (a, landed_a) in &landed {
            for (b, landed_b) in &landed {
                let (was_a, was_b) = (&arrangement[*a].geom, &arrangement[*b].geom);
                assert!(
                    moved_by(was_a, landed_a) + moved_by(was_b, landed_b)
                        <= moved_by(was_a, landed_b) + moved_by(was_b, landed_a),
                    "panes {a} and {b} came back in each other's places",
                );
            }
        }
    }
}

// utility functions
fn parse_panegeom_from_json(data_str: &str) -> PaneGeom {
    //
    // Expects this input
    //
    //  r#"{ "x": 0, "y": 1, "rows": { "constraint": "Percent(100.0)", "inner": 43 }, "cols": { "constraint": "Percent(100.0)", "inner": 211 }, "is_stacked": false }"#,
    //
    let data: HashMap<String, Value> = serde_json::from_str(data_str).unwrap();
    PaneGeom {
        x: data["x"].to_string().parse().unwrap(),
        y: data["y"].to_string().parse().unwrap(),
        rows: get_dim(&data["rows"]),
        cols: get_dim(&data["cols"]),
        stacked: None,
        is_pinned: false,
        logical_position: None,
    }
}

fn get_dim(dim_hm: &Value) -> Dimension {
    let constr_str = dim_hm["constraint"].to_string();

    if constr_str.contains("Fixed") {
        let value = &constr_str[7..constr_str.len() - 2];
        Dimension::fixed(value.parse().unwrap())
    } else if constr_str.contains("Percent") {
        let value = &constr_str[9..constr_str.len() - 2];
        let mut dim = Dimension::percent(value.parse().unwrap());
        dim.set_inner(dim_hm["inner"].to_string().parse().unwrap());
        dim
    } else {
        panic!("Constraint is nor a percent nor fixed");
    }
}
