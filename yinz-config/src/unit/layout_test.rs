use super::super::layout::*;
use insta::assert_snapshot;
use std::collections::BTreeMap;
use yinz_types::command::PaneTarget;
use yinz_types::data::PluginTag;
use yinz_types::geometry::{PercentOrFixed, SplitDirection, SplitSize};
use yinz_types::input::command::RunCommand;
use yinz_types::internal_ids::TabPosition;

fn normalize_layout_debug(s: String) -> String {
    s
}

#[test]
fn negative_pane_sizes_and_swap_constraints_are_rejected() {
    for kind in ["max_panes", "min_panes", "exact_panes"] {
        let raw = format!("layout {{ swap_tiled_layout {{ tab {kind}=2 {{ pane; }}; }}; }}");
        crate::kdl::layout::from_kdl(&raw, None, None, None).unwrap();
    }
    for size in [-1, -i64::MAX] {
        for pane in [format!("size={size}"), format!("{{ size {size}; }}")] {
            let raw = format!("layout {{ pane {pane}; }}");
            assert!(
                crate::kdl::layout::from_kdl(&raw, None, None, None).is_err(),
                "{raw}"
            );
        }
        for kind in ["max_panes", "min_panes", "exact_panes"] {
            let raw =
                format!("layout {{ swap_tiled_layout {{ tab {kind}={size} {{ pane; }}; }}; }}");
            assert!(
                crate::kdl::layout::from_kdl(&raw, None, None, None).is_err(),
                "{raw}"
            );
        }
    }
}

#[test]
fn relative_layout_directory_preserves_metadata() {
    let dir = tempfile::tempdir_in(".").unwrap();
    let relative_dir = Path::new(".").join(dir.path().file_name().unwrap());
    let file_path = relative_dir.join("review.kdl");
    std::fs::write(
        &file_path,
        "layout { tab name=\"review\" { pane; pane; }; }",
    )
    .unwrap();
    let expected_metadata = metadata_from_path(&file_path);
    assert!(!expected_metadata.tabs.is_empty());

    for layout_dir in [relative_dir, dir.path().canonicalize().unwrap()] {
        let (layouts, errors) = list_available_layouts(Some(layout_dir), &None);
        assert!(errors.is_empty());
        assert!(layouts.contains(&LayoutInfo::File(
            "review".to_owned(),
            expected_metadata.clone(),
        )));
    }
}

#[test]
fn edit_line_numbers_accept_properties_and_children_and_reject_invalid_values() {
    for line in ["line_number=42", "{ line_number 42; }"] {
        let raw = format!("layout {{ pane edit=\"notes.md\" {line}; }}");
        let layout = crate::kdl::layout::from_kdl(&raw, None, None, None).unwrap();
        assert!(matches!(
            layout.template.unwrap().0.children[0].run,
            Some(Run::EditFile(_, Some(42), _))
        ));
    }
    for pane in [
        "edit=\"notes.md\" line_number=-1",
        "edit=\"notes.md\" line_number=1.5",
        "edit=\"notes.md\" line_number=\"42\"",
        "edit=\"notes.md\" { line_number; }",
        "line_number=42",
        "command=\"sh\" line_number=42",
    ] {
        let raw = format!("layout {{ pane {pane}; }}");
        assert!(
            crate::kdl::layout::from_kdl(&raw, None, None, None).is_err(),
            "accepted invalid line number: {pane}"
        );
    }
}

#[test]
fn template_edit_line_numbers_override_or_inherit() {
    for (template_properties, floating) in [
        ("", false),
        ("size=10", false),
        ("", true),
        ("width=20", true),
    ] {
        for (template_line, inherited) in [
            ("", None),
            ("line_number=7", Some(7)),
            ("{ line_number 7; }", Some(7)),
        ] {
            for (instance, override_line) in [
                ("", None),
                ("line_number=42", Some(42)),
                ("{ line_number 42; }", Some(42)),
                ("line_number=0", Some(0)),
                ("{ line_number 0; }", Some(0)),
            ] {
                let pane = format!("editor {instance};");
                let panes = if floating {
                    format!("pane; floating_panes {{ {pane} }};")
                } else {
                    pane
                };
                let raw = format!("layout {{ pane_template name=\"editor\" edit=\"notes.md\" {template_properties} {template_line}; {panes} }}");
                let layout = crate::kdl::layout::from_kdl(&raw, None, None, None)
                    .unwrap_or_else(|error| panic!("{raw}: {error:?}"));
                let (tiled, floating_panes) = layout.template.unwrap();
                let run = if floating {
                    &floating_panes[0].run
                } else {
                    &tiled.children[0].run
                };
                assert_eq!(
                    *run,
                    Some(Run::EditFile(
                        "notes.md".into(),
                        override_line.or(inherited),
                        None
                    )),
                    "{raw}"
                );
            }
        }
    }
}

#[test]
fn template_line_numbers_reject_invalid_values_and_resolved_non_edits() {
    for (template_properties, floating) in [
        ("", false),
        ("size=10", false),
        ("", true),
        ("width=20", true),
    ] {
        for (template_run, instance) in [
            ("edit=\"notes.md\"", "line_number=-1"),
            ("edit=\"notes.md\"", "{ line_number -1; }"),
            ("edit=\"notes.md\"", "line_number=1.5"),
            ("edit=\"notes.md\"", "{ line_number 1.5; }"),
            ("edit=\"notes.md\"", "line_number=\"42\""),
            ("edit=\"notes.md\"", "{ line_number \"42\"; }"),
            ("edit=\"notes.md\"", "line_number=true"),
            ("edit=\"notes.md\"", "{ line_number; }"),
            ("edit=\"notes.md\"", "command=\"sh\" line_number=42"),
            (
                "edit=\"notes.md\"",
                "{ plugin location=\"yinz:status-bar\"; line_number 42; }",
            ),
            ("command=\"sh\"", "line_number=42"),
            ("command=\"sh\"", "{ line_number 42; }"),
            ("cwd=\"/tmp\"", "line_number=42"),
            ("", "line_number=42"),
            (
                "{ plugin location=\"yinz:status-bar\"; }",
                "{ line_number 42; }",
            ),
        ] {
            let pane = format!("editor {instance};");
            let panes = if floating {
                format!("pane; floating_panes {{ {pane} }};")
            } else {
                pane
            };
            let raw = format!("layout {{ pane_template name=\"editor\" {template_properties} {template_run}; {panes} }}");
            let error = crate::kdl::layout::from_kdl(&raw, None, None, None).expect_err(&raw);
            assert!(
                format!("{error:?}").contains("line_number"),
                "{raw}: {error:?}"
            );
        }
    }
}

#[test]
fn empty_layout() {
    let kdl_layout = "layout";
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    let expected_layout = Layout {
        template: Some((TiledPaneLayout::default(), vec![])),
        ..Default::default()
    };
    assert_eq!(layout, expected_layout);
}

#[test]
fn layout_with_one_pane() {
    let kdl_layout = r#"
        layout {
            pane
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    let expected_layout = Layout {
        template: Some((
            TiledPaneLayout {
                children: vec![TiledPaneLayout::default()],
                ..Default::default()
            },
            vec![],
        )),
        ..Default::default()
    };
    assert_eq!(layout, expected_layout);
}

#[test]
fn layout_with_multiple_panes() {
    let kdl_layout = r#"
        layout {
            pane
            pane
            pane
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    let expected_layout = Layout {
        template: Some((
            TiledPaneLayout {
                children: vec![
                    TiledPaneLayout::default(),
                    TiledPaneLayout::default(),
                    TiledPaneLayout::default(),
                ],
                ..Default::default()
            },
            vec![],
        )),
        ..Default::default()
    };
    assert_eq!(layout, expected_layout);
}

#[test]
fn layout_with_nested_panes() {
    let kdl_layout = r#"
        layout {
            pane split_direction="Vertical" {
                pane
                pane
            }
            pane {
                pane
                pane
            }
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    let expected_layout = Layout {
        template: Some((
            TiledPaneLayout {
                children: vec![
                    TiledPaneLayout {
                        children_split_direction: SplitDirection::Vertical,
                        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
                        ..Default::default()
                    },
                    TiledPaneLayout {
                        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
            vec![],
        )),
        ..Default::default()
    };
    assert_eq!(layout, expected_layout);
}

#[test]
fn layout_with_floating_panes() {
    let kdl_layout = r#"
        layout {
            floating_panes {
                pane
                pane {
                    x 10
                    y "10%"
                    width 10
                    height "10%"
                }
                pane x=10 y="10%"
                pane command="htop"
            }
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    let expected_layout = Layout {
        template: Some((
            TiledPaneLayout::default(),
            vec![
                FloatingPaneLayout::default(),
                FloatingPaneLayout {
                    x: Some(PercentOrFixed::Fixed(10)),
                    y: Some(PercentOrFixed::Percent(10)),
                    width: Some(PercentOrFixed::Fixed(10)),
                    height: Some(PercentOrFixed::Percent(10)),
                    ..Default::default()
                },
                FloatingPaneLayout {
                    x: Some(PercentOrFixed::Fixed(10)),
                    y: Some(PercentOrFixed::Percent(10)),
                    ..Default::default()
                },
                FloatingPaneLayout {
                    run: Some(Run::Command(RunCommand {
                        command: PathBuf::from("htop"),
                        hold_on_close: true,
                        ..Default::default()
                    })),
                    ..Default::default()
                },
            ],
        )),
        ..Default::default()
    };
    assert_eq!(layout, expected_layout);
}

#[test]
fn layout_with_mixed_panes_and_floating_panes() {
    let kdl_layout = r#"
        layout {
            pane
            pane
            floating_panes {
                pane
            }
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    let expected_layout = Layout {
        template: Some((
            TiledPaneLayout {
                children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
                ..Default::default()
            },
            vec![FloatingPaneLayout::default()],
        )),
        ..Default::default()
    };
    assert_eq!(layout, expected_layout);
}

#[test]
fn layout_with_hidden_floating_panes() {
    let kdl_layout = r#"
        layout {
            tab hide_floating_panes=true {
                pane
                pane
                floating_panes {
                    pane
                }
            }
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    let expected_layout = Layout {
        tabs: vec![(
            None,
            TiledPaneLayout {
                children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
                hide_floating_panes: true,
                ..Default::default()
            },
            vec![FloatingPaneLayout::default()],
        )],
        template: Some((TiledPaneLayout::default(), vec![])),
        ..Default::default()
    };
    assert_eq!(layout, expected_layout);
}

/// What session serialization writes for a tab the user renamed.
#[test]
fn layout_with_a_user_named_tab() {
    let kdl_layout = r#"
        layout {
            tab name="notes" user_named=true {
                pane
            }
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    let (name, tab_layout, _) = layout.tabs.first().unwrap();
    assert_eq!(name.as_deref(), Some("notes"));
    assert!(tab_layout.user_named);
}

#[test]
fn layout_with_floating_panes_template() {
    let kdl_layout = r#"
        layout {
            pane_template name="my_cool_template" {
                x 10
                y "10%"
            }
            pane
            floating_panes {
                pane
                my_cool_template
            }
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    let expected_layout = Layout {
        template: Some((
            TiledPaneLayout {
                children: vec![TiledPaneLayout::default()],
                ..Default::default()
            },
            vec![
                FloatingPaneLayout::default(),
                FloatingPaneLayout {
                    x: Some(PercentOrFixed::Fixed(10)),
                    y: Some(PercentOrFixed::Percent(10)),
                    ..Default::default()
                },
            ],
        )),
        ..Default::default()
    };
    assert_eq!(layout, expected_layout);
}

#[test]
fn layout_with_shared_tiled_and_floating_panes_template() {
    let kdl_layout = r#"
        layout {
            pane_template name="htop" {
                command "htop"
            }
            htop
            floating_panes {
                pane
                htop
            }
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    let expected_layout = Layout {
        template: Some((
            TiledPaneLayout {
                children: vec![TiledPaneLayout {
                    run: Some(Run::Command(RunCommand {
                        command: PathBuf::from("htop"),
                        hold_on_close: true,
                        ..Default::default()
                    })),
                    ..Default::default()
                }],
                ..Default::default()
            },
            vec![
                FloatingPaneLayout::default(),
                FloatingPaneLayout {
                    run: Some(Run::Command(RunCommand {
                        command: PathBuf::from("htop"),
                        hold_on_close: true,
                        ..Default::default()
                    })),
                    ..Default::default()
                },
            ],
        )),
        ..Default::default()
    };
    assert_eq!(layout, expected_layout);
}

#[test]
fn layout_with_tabs_and_floating_panes() {
    let kdl_layout = r#"
        layout {
            tab {
                floating_panes {
                    pane
                }
            }
            tab {
                floating_panes {
                    pane
                    pane
                }
            }
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn layout_with_tabs() {
    let kdl_layout = r#"
        layout {
            tab
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    let expected_layout = Layout {
        tabs: vec![(None, TiledPaneLayout::default(), vec![])],
        template: Some((TiledPaneLayout::default(), vec![])),
        ..Default::default()
    };
    assert_eq!(layout, expected_layout);
}

#[test]
fn layout_with_nested_differing_tabs() {
    let kdl_layout = r#"
        layout {
            tab split_direction="Vertical" {
                pane
                pane
                pane
            }
            tab {
                pane
                pane
            }
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    let expected_layout = Layout {
        tabs: vec![
            (
                None,
                TiledPaneLayout {
                    children_split_direction: SplitDirection::Vertical,
                    children: vec![
                        TiledPaneLayout::default(),
                        TiledPaneLayout::default(),
                        TiledPaneLayout::default(),
                    ],
                    ..Default::default()
                },
                vec![], // floating panes
            ),
            (
                None,
                TiledPaneLayout {
                    children_split_direction: SplitDirection::Horizontal,
                    children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
                    ..Default::default()
                },
                vec![], // floating panes
            ),
        ],
        template: Some((TiledPaneLayout::default(), vec![])),
        ..Default::default()
    };
    assert_eq!(layout, expected_layout);
}

#[test]
fn layout_with_panes_in_different_mixed_split_sizes() {
    let kdl_layout = r#"
        layout {
            pane size=1;
            pane size="10%";
            pane;
            pane size=2;
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    let expected_layout = Layout {
        template: Some((
            TiledPaneLayout {
                children: vec![
                    TiledPaneLayout {
                        split_size: Some(SplitSize::Fixed(1)),
                        ..Default::default()
                    },
                    TiledPaneLayout {
                        split_size: Some(SplitSize::Percent(10)),
                        ..Default::default()
                    },
                    TiledPaneLayout {
                        split_size: None,
                        ..Default::default()
                    },
                    TiledPaneLayout {
                        split_size: Some(SplitSize::Fixed(2)),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
            vec![],
        )),
        ..Default::default()
    };
    assert_eq!(layout, expected_layout);
}

#[test]
fn layout_with_command_panes() {
    let kdl_layout = r#"
        layout {
            pane command="htop"
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    let expected_layout = Layout {
        template: Some((
            TiledPaneLayout {
                children: vec![TiledPaneLayout {
                    run: Some(Run::Command(RunCommand {
                        command: PathBuf::from("htop"),
                        hold_on_close: true,
                        ..Default::default()
                    })),
                    ..Default::default()
                }],
                ..Default::default()
            },
            vec![],
        )),
        ..Default::default()
    };
    assert_eq!(layout, expected_layout);
}

#[test]
fn layout_with_command_panes_and_cwd() {
    let kdl_layout = r#"
        layout {
            pane command="htop" cwd="/path/to/my/cwd"
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    let expected_layout = Layout {
        template: Some((
            TiledPaneLayout {
                children: vec![TiledPaneLayout {
                    run: Some(Run::Command(RunCommand {
                        command: PathBuf::from("htop"),
                        cwd: Some(PathBuf::from("/path/to/my/cwd")),
                        hold_on_close: true,
                        ..Default::default()
                    })),
                    ..Default::default()
                }],
                ..Default::default()
            },
            vec![],
        )),
        ..Default::default()
    };
    assert_eq!(layout, expected_layout);
}

#[test]
fn layout_with_command_panes_and_cwd_and_args() {
    let kdl_layout = r#"
        layout {
            pane command="htop" cwd="/path/to/my/cwd" {
                args "-h" "-v"
            }
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    let expected_layout = Layout {
        template: Some((
            TiledPaneLayout {
                children: vec![TiledPaneLayout {
                    run: Some(Run::Command(RunCommand {
                        command: PathBuf::from("htop"),
                        cwd: Some(PathBuf::from("/path/to/my/cwd")),
                        args: vec![String::from("-h"), String::from("-v")],
                        hold_on_close: true,
                        ..Default::default()
                    })),
                    ..Default::default()
                }],
                ..Default::default()
            },
            vec![],
        )),
        ..Default::default()
    };
    assert_eq!(layout, expected_layout);
}

#[test]
fn layout_with_command_panes_and_close_on_exit() {
    let kdl_layout = r#"
        layout {
            pane command="htop" {
                close_on_exit true
            }
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn layout_with_command_panes_and_start_suspended() {
    let kdl_layout = r#"
        layout {
            pane command="htop" {
                start_suspended true
            }
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn layout_with_plugin_panes() {
    let kdl_layout = r#"
        layout {
            pane {
                plugin location="yinz:tab-bar"
            }
            pane {
                plugin location="file:/path/to/my/plugin.scm"
            }
            pane {
                plugin location="yinz:status-bar" {
                    config_key_1 "config_value_1"
                    "2" true
                }
            }
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    let mut expected_plugin_configuration = BTreeMap::new();
    expected_plugin_configuration.insert("config_key_1".to_owned(), "config_value_1".to_owned());
    expected_plugin_configuration.insert("2".to_owned(), "true".to_owned());
    let expected_layout = Layout {
        template: Some((
            TiledPaneLayout {
                children: vec![
                    TiledPaneLayout {
                        run: Some(Run::Plugin(RunPluginOrAlias::RunPlugin(RunPlugin {
                            location: RunPluginLocation::Yinz(PluginTag::new("tab-bar")),
                            _allow_exec_host_cmd: false,
                            configuration: Default::default(),
                            ..Default::default()
                        }))),
                        ..Default::default()
                    },
                    TiledPaneLayout {
                        run: Some(Run::Plugin(RunPluginOrAlias::RunPlugin(RunPlugin {
                            location: RunPluginLocation::File(PathBuf::from(
                                "/path/to/my/plugin.scm",
                            )),
                            _allow_exec_host_cmd: false,
                            configuration: Default::default(),
                            ..Default::default()
                        }))),
                        ..Default::default()
                    },
                    TiledPaneLayout {
                        run: Some(Run::Plugin(RunPluginOrAlias::RunPlugin(RunPlugin {
                            location: RunPluginLocation::Yinz(PluginTag::new("status-bar")),
                            _allow_exec_host_cmd: false,
                            configuration: PluginUserConfiguration::new(
                                expected_plugin_configuration,
                            ),
                            ..Default::default()
                        }))),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
            vec![],
        )),
        ..Default::default()
    };
    assert_eq!(layout, expected_layout);
}

#[test]
fn layout_with_borderless_panes() {
    let kdl_layout = r#"
        layout {
            pane borderless=true
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    let expected_layout = Layout {
        template: Some((
            TiledPaneLayout {
                children: vec![TiledPaneLayout {
                    borderless: Some(true),
                    ..Default::default()
                }],
                ..Default::default()
            },
            vec![],
        )),
        ..Default::default()
    };
    assert_eq!(layout, expected_layout);
}

#[test]
fn layout_with_focused_panes() {
    let kdl_layout = r#"
        layout {
            pane focus=true
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    let expected_layout = Layout {
        template: Some((
            TiledPaneLayout {
                children: vec![TiledPaneLayout {
                    focus: Some(true),
                    ..Default::default()
                }],
                ..Default::default()
            },
            vec![],
        )),
        ..Default::default()
    };
    assert_eq!(layout, expected_layout);
}

#[test]
fn layout_with_pane_names() {
    let kdl_layout = r#"
        layout {
            pane name="my awesome pane"
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    let expected_layout = Layout {
        template: Some((
            TiledPaneLayout {
                children: vec![TiledPaneLayout {
                    name: Some("my awesome pane".into()),
                    ..Default::default()
                }],
                ..Default::default()
            },
            vec![],
        )),
        ..Default::default()
    };
    assert_eq!(layout, expected_layout);
}

#[test]
fn layout_with_tab_names() {
    let kdl_layout = r#"
        layout {
            tab name="my cool tab name 1"
            tab name="my cool tab name 2"
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    let expected_layout = Layout {
        tabs: vec![
            (
                Some("my cool tab name 1".into()),
                TiledPaneLayout {
                    children: vec![],
                    ..Default::default()
                },
                vec![], // floating panes
            ),
            (
                Some("my cool tab name 2".into()),
                TiledPaneLayout {
                    children: vec![],
                    ..Default::default()
                },
                vec![], // floating panes
            ),
        ],
        template: Some((TiledPaneLayout::default(), vec![])),
        ..Default::default()
    };
    assert_eq!(layout, expected_layout);
}

#[test]
fn layout_with_focused_tab() {
    let kdl_layout = r#"
        layout {
            tab
            tab focus=true
            tab
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    let expected_layout = Layout {
        tabs: vec![
            (None, TiledPaneLayout::default(), vec![]),
            (None, TiledPaneLayout::default(), vec![]),
            (None, TiledPaneLayout::default(), vec![]),
        ],
        template: Some((TiledPaneLayout::default(), vec![])),
        focused_tab_index: Some(TabPosition::new(1)),
        ..Default::default()
    };
    assert_eq!(layout, expected_layout);
}

#[test]
fn layout_with_tab_templates() {
    let kdl_layout = r#"
        layout {
            tab_template name="one-above-one-below" {
                pane
                children
                pane
            }
            one-above-one-below name="my first tab" split_direction="Vertical" {
                pane
                pane
            }
            one-above-one-below name="my second tab" {
                pane
                pane
            }
            one-above-one-below
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    let expected_layout = Layout {
        tabs: vec![
            (
                Some("my first tab".into()),
                TiledPaneLayout {
                    children_split_direction: SplitDirection::Horizontal,
                    children: vec![
                        TiledPaneLayout::default(),
                        TiledPaneLayout {
                            children_split_direction: SplitDirection::Vertical,
                            children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
                            ..Default::default()
                        },
                        TiledPaneLayout::default(),
                    ],
                    ..Default::default()
                },
                vec![], // floating panes
            ),
            (
                Some("my second tab".into()),
                TiledPaneLayout {
                    children_split_direction: SplitDirection::Horizontal,
                    children: vec![
                        TiledPaneLayout::default(),
                        TiledPaneLayout {
                            children_split_direction: SplitDirection::Horizontal,
                            children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
                            ..Default::default()
                        },
                        TiledPaneLayout::default(),
                    ],
                    ..Default::default()
                },
                vec![], // floating panes
            ),
            (
                None,
                TiledPaneLayout {
                    children_split_direction: SplitDirection::Horizontal,
                    children: vec![
                        TiledPaneLayout::default(),
                        TiledPaneLayout::default(),
                        TiledPaneLayout::default(),
                    ],
                    ..Default::default()
                },
                vec![], // floating panes
            ),
        ],
        template: Some((TiledPaneLayout::default(), vec![])),
        ..Default::default()
    };
    assert_eq!(layout, expected_layout);
}

#[test]
fn tab_group_metadata_is_preserved_with_and_without_templates() {
    for (template, node) in [
        ("", "tab"),
        ("default_tab_template { children; }", "tab"),
        ("tab_template name=\"custom\" { children; }", "custom"),
    ] {
        for (metadata, expected_group, expected_focus) in [
            ("", None, false),
            ("group_id=0", Some((0, "Default")), false),
            ("group_id=4294967295", Some((u32::MAX, "Default")), false),
            (
                "group_id=7 group_name=\"工程\" group_focused=true",
                Some((7, "工程")),
                true,
            ),
        ] {
            let raw = format!("layout {{ {template}\n{node} {metadata}; }}");
            let layout = crate::kdl::layout::from_kdl(&raw, None, None, None).unwrap();
            let tab = &layout.tabs[0].1;
            assert_eq!(
                tab.tab_group.as_ref().map(|g| (g.id, g.name.as_str())),
                expected_group,
                "{raw}"
            );
            assert_eq!(tab.group_focused, expected_focus, "{raw}");
        }
    }
}

#[test]
fn tab_group_ids_are_validated_with_and_without_templates() {
    for (template, node) in [
        ("", "tab"),
        ("default_tab_template { children; }", "tab"),
        ("tab_template name=\"custom\" { children; }", "custom"),
    ] {
        for (id, message) in [
            ("-1", "Group ID is out of range"),
            ("4294967296", "Group ID is out of range"),
            ("\"7\"", "Group ID must be an integer"),
            ("1.5", "Group ID must be an integer"),
            ("true", "Group ID must be an integer"),
        ] {
            let raw = format!("layout {{ {template}\n{node} group_id={id}; }}");
            let error = crate::kdl::layout::from_kdl(&raw, None, None, None).unwrap_err();
            assert!(format!("{error:?}").contains(message), "{raw}: {error:?}");
        }
    }
}

#[test]
fn layout_with_default_tab_template() {
    let kdl_layout = r#"
        layout {
            default_tab_template {
                pane
                children
                pane
            }
            tab name="my first tab" split_direction="Vertical" {
                pane
                pane
            }
            tab name="my second tab" {
                pane
                pane
            }
            tab
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn layout_with_new_tab_template() {
    let kdl_layout = r#"
        layout {
            new_tab_template {
                pane split_direction="vertical" {
                    pane
                    pane
                }
            }
            tab name="my first tab" split_direction="Vertical" {
                pane
                pane
            }
            tab name="my second tab" {
                pane
                pane
            }
            tab
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn plugin_pane_template_keeps_the_plugin_when_the_consumer_sets_a_cwd() {
    // A consumer that only adds `cwd` to a plugin template used to come back as
    // `Run::Cwd`: the plugin was dropped and the pane opened a shell instead.
    for location in ["strider", "yinz:strider"] {
        let raw = format!(
            r#"
            layout {{
                pane_template name="files" {{ plugin location="{location}"; }}
                files cwd="/tmp"
                files
            }}
        "#
        );
        let layout = crate::kdl::layout::from_kdl(&raw, None, None, None).unwrap();
        let (tiled, _) = layout.template.unwrap();
        let with_cwd = tiled.children[0].run.as_ref().expect("a run");
        let without_cwd = tiled.children[1].run.as_ref().expect("a run");
        match with_cwd {
            Run::Plugin(plugin) => {
                assert_eq!(plugin.get_initial_cwd(), Some(PathBuf::from("/tmp")));
                assert_eq!(plugin.location_string(), location);
            },
            other => panic!("expected the template's plugin with a cwd, got {other:?}"),
        }
        match without_cwd {
            Run::Plugin(plugin) => assert_eq!(plugin.get_initial_cwd(), None),
            other => panic!("expected the template's plugin, got {other:?}"),
        }
    }
}

#[test]
fn cwd_only_pane_template_gives_its_cwd_to_a_plugin_consumer() {
    for location in ["strider", "yinz:strider"] {
        let raw = format!(
            r#"
            layout {{
                pane_template name="in_template" cwd="/template"
                in_template {{ plugin location="{location}" cwd="/consumer"; }}
                in_template {{ plugin location="{location}"; }}
            }}
        "#
        );
        let layout = crate::kdl::layout::from_kdl(&raw, None, None, None).unwrap();
        let (tiled, _) = layout.template.unwrap();
        for (child, cwd) in tiled.children.iter().zip(["/consumer", "/template"]) {
            match child.run.as_ref().expect("a run") {
                Run::Plugin(plugin) => {
                    assert_eq!(plugin.get_initial_cwd(), Some(PathBuf::from(cwd)));
                    assert_eq!(plugin.location_string(), location);
                },
                other => panic!("expected a plugin with cwd {cwd}, got {other:?}"),
            }
        }
    }
}

#[test]
fn plugin_alias_initial_cwd_precedes_resolved_cwd() {
    for resolved in [false, true] {
        for cwd in [None, Some(PathBuf::from("/consumer"))] {
            let mut alias = PluginAlias::new("strider", &None, cwd.clone());
            if resolved {
                alias.run_plugin = Some(RunPlugin {
                    initial_cwd: Some(PathBuf::from("/resolved")),
                    ..Default::default()
                });
            }
            assert_eq!(
                RunPluginOrAlias::Alias(alias).get_initial_cwd(),
                cwd.or_else(|| resolved.then(|| PathBuf::from("/resolved")))
            );
        }
    }
}

#[test]
fn edit_pane_template_follows_the_consumer_cwd() {
    // The parser anchors a template's `edit` to the template's cwd, so by the time the
    // consumer's cwd is merged in the file is already an absolute path; joining onto it
    // was a no-op, and the editor opened the template's file from the consumer's cwd.
    let raw = r#"
        layout {
            pane_template name="notes" edit="notes.md" cwd="/a"
            pane_template name="loose_notes" edit="notes.md"
            notes cwd="/b"
            notes
            loose_notes cwd="/b"
        }
    "#;
    let layout = crate::kdl::layout::from_kdl(raw, None, None, None).unwrap();
    let (tiled, _) = layout.template.unwrap();
    let runs: Vec<_> = tiled
        .children
        .iter()
        .map(|child| child.run.clone().expect("a run"))
        .collect();
    assert_eq!(
        runs,
        vec![
            Run::EditFile(
                PathBuf::from("/b/notes.md"),
                None,
                Some(PathBuf::from("/b"))
            ),
            Run::EditFile(
                PathBuf::from("/a/notes.md"),
                None,
                Some(PathBuf::from("/a"))
            ),
            Run::EditFile(
                PathBuf::from("/b/notes.md"),
                None,
                Some(PathBuf::from("/b"))
            ),
        ]
    );
}

#[test]
fn an_absolute_edit_in_a_pane_template_stays_put_when_the_consumer_sets_a_cwd() {
    let raw = r#"
        layout {
            pane_template name="hosts" edit="/etc/hosts" cwd="/a"
            hosts cwd="/b"
        }
    "#;
    let layout = crate::kdl::layout::from_kdl(raw, None, None, None).unwrap();
    let (tiled, _) = layout.template.unwrap();
    assert_eq!(
        tiled.children[0].run,
        Some(Run::EditFile(
            PathBuf::from("/etc/hosts"),
            None,
            Some(PathBuf::from("/b"))
        ))
    );
}

#[test]
fn layout_with_pane_templates() {
    let kdl_layout = r#"
        layout {
            pane_template name="left-and-right" split_direction="Vertical" {
                pane
                children
                pane
            }
            left-and-right {
                pane
            }
            left-and-right {
                pane
                pane
            }
            left-and-right split_direction="Vertical" {
                pane
                pane
            }
            left-and-right
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn layout_with_tab_and_pane_templates() {
    let kdl_layout = r#"
        layout {
            tab_template name="left-right-and-htop" {
                left-and-right {
                    pane command="htop"
                }
            }
            pane_template name="left-and-right" split_direction="Vertical" {
                pane
                children
                pane
            }
            left-right-and-htop
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn layout_with_nested_pane_templates() {
    let kdl_layout = r#"
        layout {
            pane_template name="left-and-right" split_direction="Vertical" {
                pane
                children
                up-and-down
                pane
            }
            pane_template name="up-and-down" split_direction="Horizontal" {
                pane
                pane
            }
            left-and-right
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn layout_with_nested_branched_pane_templates() {
    let kdl_layout = r#"
        layout {
            pane_template name="left-and-right" split_direction="Vertical" {
                pane
                children
                up-and-down
                three-horizontal-panes
            }
            pane_template name="three-horizontal-panes" split_direction="Horizontal" {
                pane
                pane
                pane
            }
            pane_template name="up-and-down" split_direction="Horizontal" {
                pane
                pane
            }
            left-and-right
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn circular_dependency_pane_templates_error() {
    let kdl_layout = r#"
        layout {
            pane_template name="one" split_direction="Vertical" {
                pane
                children
                two
            }
            pane_template name="two" split_direction="Horizontal" {
                pane
                three
                pane
            }
            pane_template name="three" split_direction="Horizontal" {
                one
            }
            one
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None);
    assert!(layout.is_err(), "circular dependency detected");
}

#[test]
fn children_not_as_first_child_of_tab_template() {
    let kdl_layout = r#"
        layout {
            tab_template name="horizontal-with-vertical-top" {
                pane split_direction="Vertical" {
                    pane
                    children
                }
                pane
            }
            horizontal-with-vertical-top name="my tab" {
                pane
                pane
            }
            horizontal-with-vertical-top
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn error_on_more_than_one_children_block_in_tab_template() {
    let kdl_layout = r#"
        layout {
            tab_template name="horizontal-with-vertical-top" {
                pane split_direction="Vertical" {
                    pane
                    children
                }
                children
                pane
            }
            horizontal-with-vertical-top name="my tab" {
                pane
                pane
            }
            horizontal-with-vertical-top
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None);
    assert!(
        layout.is_err(),
        "error provided for more than one children block"
    );
}

#[test]
fn children_not_as_first_child_of_pane_template() {
    let kdl_layout = r#"
        layout {
            pane_template name="horizontal-with-vertical-top" {
                pane split_direction="Vertical" {
                    pane
                    children
                }
                pane
            }
            horizontal-with-vertical-top name="my pane" {
                pane
                pane
            }
            horizontal-with-vertical-top
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn error_on_more_than_one_children_block_in_pane_template() {
    let kdl_layout = r#"
        layout {
            pane_template name="horizontal-with-vertical-top" {
                pane split_direction="Vertical" {
                    pane
                    children
                }
                children
                pane
            }
            horizontal-with-vertical-top name="my tab" {
                pane
                pane
            }
            horizontal-with-vertical-top
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None);
    assert!(
        layout.is_err(),
        "error provided for more than one children block"
    );
}

#[test]
fn combined_tab_and_pane_template_both_with_children() {
    let kdl_layout = r#"
        layout {
            tab_template name="horizontal-with-vertical-top" {
                vertical-sandwich {
                    pane name="middle"
                }
                children
            }
            pane_template name="vertical-sandwich" split_direction="Vertical" {
                pane
                children
                pane
            }
            horizontal-with-vertical-top name="my tab" {
                pane
                pane
            }
            horizontal-with-vertical-top
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn layout_with_pane_excluded_from_sync() {
    let kdl_layout = r#"
        layout {
            pane exclude_from_sync=true
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn cannot_define_tab_template_name_with_space() {
    let kdl_layout = r#"
        layout {
            tab_template name="with space" {
                pane
                children
                pane
            }
            pane
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None);
    assert!(layout.is_err(), "error provided for tab name with space");
}

#[test]
fn cannot_define_pane_template_name_with_space() {
    let kdl_layout = r#"
        layout {
            pane_template name="with space" {
                pane
                children
                pane
            }
            pane
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None);
    assert!(layout.is_err(), "error provided for tab name with space");
}

#[test]
fn cannot_define_panes_and_tabs_on_same_level() {
    let kdl_layout = r#"
        layout {
            pane
            tab {
                pane
            }
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None);
    assert!(
        layout.is_err(),
        "error provided for tab and pane on the same level"
    );
}

#[test]
fn cannot_define_tab_template_names_as_keywords() {
    let keywords = vec![
        "pane",
        "layout",
        "pane_template",
        "tab_template",
        "default_tab_template",
        "command",
        "plugin",
        "children",
        "tab",
    ];
    for keyword in keywords {
        let kdl_layout = format!(
            "
            layout {{
                tab_template name=\"{}\" {{
                    pane
                    children
                    pane
                }}
                pane
            }}
        ",
            keyword
        );
        let layout =
            crate::kdl::layout::from_kdl(&kdl_layout, Some("layout_file_name".into()), None, None);
        assert!(
            layout.is_err(),
            "{}",
            format!(
                "error provided for tab template name with keyword: {}",
                keyword
            )
        );
    }
}

#[test]
fn cannot_define_pane_template_names_as_keywords() {
    let keywords = vec![
        "pane",
        "layout",
        "pane_template",
        "tab_template",
        "command",
        "plugin",
        "children",
        "tab",
    ];
    for keyword in keywords {
        let kdl_layout = format!(
            "
            layout {{
                pane_template name=\"{}\" {{
                    pane
                    children
                    pane
                }}
                pane
            }}
        ",
            keyword
        );
        let layout =
            crate::kdl::layout::from_kdl(&kdl_layout, Some("layout_file_name".into()), None, None);
        assert!(
            layout.is_err(),
            "{}",
            format!(
                "error provided for pane template name with keyword: {}",
                keyword
            )
        );
    }
}

#[test]
fn error_on_multiple_layout_nodes_in_file() {
    let kdl_layout = "
        layout
        layout
    "
    .to_string();
    let layout_error =
        crate::kdl::layout::from_kdl(&kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap_err();
    assert_snapshot!(normalize_layout_debug(format!("{:?}", layout_error)));
}

#[test]
fn error_on_unknown_layout_node() {
    let kdl_layout = "
        layout {
            pane
            i_am_not_a_proper_node
            pane
        }
    "
    .to_string();
    let layout_error =
        crate::kdl::layout::from_kdl(&kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap_err();
    assert_snapshot!(normalize_layout_debug(format!("{:?}", layout_error)));
}

#[test]
fn error_on_unknown_layout_pane_property() {
    let kdl_layout = "
        layout {
            pane spit_size=1
        }
    "
    .to_string();
    let layout_error =
        crate::kdl::layout::from_kdl(&kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap_err();
    assert_snapshot!(normalize_layout_debug(format!("{:?}", layout_error)));
}

#[test]
fn error_on_unknown_layout_pane_template_property() {
    let kdl_layout = "
        layout {
            pane_template name=\"my_cool_template\" spit_size=1
        }
    "
    .to_string();
    let layout_error =
        crate::kdl::layout::from_kdl(&kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap_err();
    assert_snapshot!(normalize_layout_debug(format!("{:?}", layout_error)));
}

#[test]
fn error_on_unknown_layout_tab_property() {
    let kdl_layout = "
        layout {
            tab spit_size=1
        }
    "
    .to_string();
    let layout_error =
        crate::kdl::layout::from_kdl(&kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap_err();
    assert_snapshot!(normalize_layout_debug(format!("{:?}", layout_error)));
}

#[test]
fn error_on_unknown_layout_tab_template_property() {
    let kdl_layout = "
        layout {
            tab_template name=\"my_cool_template\" spit_size=1
        }
    "
    .to_string();
    let layout_error =
        crate::kdl::layout::from_kdl(&kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap_err();
    assert_snapshot!(normalize_layout_debug(format!("{:?}", layout_error)));
}

#[test]
fn error_on_pane_templates_without_a_name() {
    let kdl_layout = "
        layout {
            pane_template {
                pane
                children
                pane
            }
        }
    "
    .to_string();
    let layout_error =
        crate::kdl::layout::from_kdl(&kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap_err();
    assert_snapshot!(normalize_layout_debug(format!("{:?}", layout_error)));
}

#[test]
fn error_on_tab_templates_without_a_name() {
    let kdl_layout = "
        layout {
            tab_template {
                pane
                children
                pane
            }
        }
    "
    .to_string();
    let layout_error =
        crate::kdl::layout::from_kdl(&kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap_err();
    assert_snapshot!(normalize_layout_debug(format!("{:?}", layout_error)));
}

#[test]
fn error_on_more_than_one_focused_tab() {
    let kdl_layout = r#"
        layout {
            tab focus=true
            tab focus=true
            tab
        }
    "#;
    let layout_error =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap_err();
    assert_snapshot!(normalize_layout_debug(format!("{:?}", layout_error)));
}

#[test]
fn args_override_args_in_template() {
    let kdl_layout = r#"
        layout {
            pane_template name="tail" {
                command "tail"
                args "-f" "/tmp/foo"
            }
            tail
            tail {
                args "-f" "/tmp/bar"
            }
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn close_on_exit_overrides_close_on_exit_in_template() {
    let kdl_layout = r#"
        layout {
            pane_template name="tail" {
                command "tail"
                close_on_exit false
            }
            tail
            tail {
                close_on_exit true
            }
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn args_added_to_args_in_template() {
    let kdl_layout = r#"
        layout {
            pane_template name="tail" {
                command "tail"
            }
            tail
            tail {
                args "-f" "/tmp/bar"
            }
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn close_on_exit_added_to_close_on_exit_in_template() {
    let kdl_layout = r#"
        layout {
            pane_template name="tail" {
                command "tail"
            }
            tail
            tail {
                close_on_exit true
            }
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn cwd_override_cwd_in_template() {
    let kdl_layout = r#"
        layout {
            pane_template name="tail" {
                command "tail"
                cwd "/tmp"
            }
            tail
            tail {
                cwd "/"
            }
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn cwd_added_to_cwd_in_template() {
    let kdl_layout = r#"
        layout {
            pane_template name="tail" {
                command "tail"
            }
            tail
            tail {
                cwd "/home"
            }
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn error_on_mixed_command_and_child_panes() {
    let kdl_layout = r#"
        layout {
            pane command="tail" {
                pane
            }
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None);
    assert!(layout.is_err(), "error provided");
}

#[test]
fn error_on_mixed_cwd_and_child_panes() {
    let kdl_layout = r#"
        layout {
            pane cwd="/tmp" {
                pane
                pane
            }
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None);
    assert!(layout.is_err(), "error provided");
}

#[test]
fn error_on_bare_args_without_command() {
    let kdl_layout = r#"
        layout {
            pane {
                args "-f"
            }
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None);
    assert!(layout.is_err(), "error provided");
}

#[test]
fn error_on_bare_close_on_exit_without_command() {
    let kdl_layout = r#"
        layout {
            pane {
                close_on_exit true
            }
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None);
    assert!(layout.is_err(), "error provided");
}

#[test]
fn error_on_bare_args_in_template_without_command() {
    let kdl_layout = r#"
        layout {
            pane_template name="my_template"
            my_template {
                args "--help"
            }
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None);
    assert!(layout.is_err(), "error provided");
}

#[test]
fn error_on_bare_close_on_exit_in_template_without_command() {
    let kdl_layout = r#"
        layout {
            pane_template name="my_template"
            my_template {
                close_on_exit true
            }
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None);
    assert!(layout.is_err(), "error provided");
}

#[test]
fn pane_template_command_with_cwd_overriden_by_its_consumers_command_cwd() {
    let kdl_layout = r#"
        layout {
            cwd "/tmp"
            pane_template name="tail" {
                command "tail"
                cwd "bar"
            }
            tail command="pwd" {
                cwd "foo"
            }
            // pane should have /tmp/foo and not /tmp/bar as cwd
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn pane_template_command_with_cwd_remains_when_its_consumer_command_does_not_have_a_cwd() {
    let kdl_layout = r#"
        layout {
            cwd "/tmp"
            pane_template name="tail" {
                command "tail"
                cwd "bar"
            }
            tail command="pwd"
            // pane should have /tmp/bar as its cwd with the pwd command
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn pane_template_command_without_cwd_is_overriden_by_its_consumers_cwd() {
    let kdl_layout = r#"
        layout {
            cwd "/tmp"
            pane_template name="tail" {
                command "tail"
            }
            tail command="pwd" {
                cwd "bar"
            }
            // pane should have /tmp/bar as its cwd with the pwd command
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn pane_template_command_with_cwd_is_overriden_by_its_consumers_bare_cwd() {
    let kdl_layout = r#"
        layout {
            cwd "/tmp"
            pane_template name="tail" {
                command "tail"
                cwd "foo"
            }
            tail {
                cwd "bar"
            }
            // pane should have /tmp/bar as its cwd with the tail command
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn pane_template_command_without_cwd_receives_its_consumers_bare_cwd() {
    let kdl_layout = r#"
        layout {
            cwd "/tmp"
            pane_template name="tail" {
                command "tail"
            }
            tail {
                cwd "bar"
            }
            // pane should have /tmp/bar as its cwd with the tail command
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn pane_template_with_bare_cwd_overriden_by_its_consumers_bare_cwd() {
    let kdl_layout = r#"
        layout {
            cwd "/tmp"
            pane_template name="tail" {
                cwd "foo"
            }
            tail {
                cwd "bar"
            }
            // pane should have /tmp/foo without a command
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn pane_template_with_bare_propagated_to_its_consumer_command_without_cwd() {
    let kdl_layout = r#"
        layout {
            cwd "/tmp"
            pane_template name="tail" {
                cwd "foo"
            }
            tail command="tail"
            // pane should have /tmp/foo with the tail command
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn pane_template_with_bare_propagated_to_its_consumer_command_with_cwd() {
    let kdl_layout = r#"
        layout {
            cwd "/tmp"
            pane_template name="tail" {
                cwd "foo"
            }
            tail command="tail" {
                cwd "bar"
            }
            // pane should have /tmp/bar with the tail command
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn pane_template_with_bare_propagated_to_its_consumer_edit() {
    let kdl_layout = r#"
        layout {
            cwd "/tmp"
            pane_template name="tail" {
                cwd "foo"
            }
            tail edit="bar"
            // pane should have /tmp/foo/bar with the edit file variant
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn pane_template_with_command_propagated_to_its_consumer_edit() {
    let kdl_layout = r#"
        layout {
            cwd "/tmp"
            pane_template name="tail" command="not-vim" {
                cwd "foo"
            }
            tail edit="bar"
            // pane should have /tmp/foo/bar with the edit file variant
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn global_cwd_given_to_panes_without_cwd() {
    let kdl_layout = r#"
        layout {
            cwd "/tmp"
            pane
            pane command="tail"
            // both should have the /tmp cwd
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn global_cwd_prepended_to_panes_with_cwd() {
    let kdl_layout = r#"
        layout {
            cwd "/tmp"
            pane cwd="foo" // should be /tmp/foo
            pane command="tail" cwd="/home/foo" // should be /home/foo because its an absolute path
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn global_cwd_passed_from_layout_constructor() {
    // this is used by the new-tab cli action with --cwd
    let kdl_layout = r#"
        layout {
            pane
            pane command="tail"
            // both should have the /tmp cwd
        }
    "#;
    let layout = crate::kdl::layout::from_kdl(
        kdl_layout,
        Some("layout_file_name".into()),
        None,
        Some(PathBuf::from("/tmp")),
    )
    .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn global_cwd_passed_from_layout_constructor_overrides_global_cwd_in_layout_file() {
    // this is used by the new-tab cli action with --cwd
    let kdl_layout = r#"
        layout {
            cwd "/home"
            pane
            pane command="tail"
            // both should have the /tmp cwd
        }
    "#;
    let layout = crate::kdl::layout::from_kdl(
        kdl_layout,
        Some("layout_file_name".into()),
        None,
        Some(PathBuf::from("/tmp")),
    )
    .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn global_cwd_with_tab_cwd_given_to_panes_without_cwd() {
    let kdl_layout = r#"
        layout {
            cwd "/tmp"
            tab cwd="./foo" {
                pane
                pane command="tail"
            }
            // both should have the /tmp/foo cwd
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn tab_cwd_given_to_panes_without_cwd() {
    let kdl_layout = r#"
        layout {
            tab cwd="/tmp" {
                pane
                pane command="tail"
            }
            // both should have the /tmp cwd
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn tab_cwd_prepended_to_panes_with_cwd() {
    let kdl_layout = r#"
        layout {
            tab cwd="/tmp" {
                pane cwd="./foo"
                pane command="tail" cwd="./foo"
            }
            // both should have the /tmp/foo cwd
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

/// A pane on an ssh host, which has no layout spelling but reaches
/// `Run::add_cwd` through the same layout, tab and template prefixes, gets
/// the prefix like a local pane: the layout names where it starts there.
#[test]
fn global_cwd_prepended_to_remote_host_pane_with_cwd() {
    use yinz_types::input::command::RunCommand;
    let mut run = Run::Command(RunCommand {
        command: PathBuf::from("cargo"),
        cwd: Some(PathBuf::from("src/project")),
        target: PaneTarget::Ssh {
            host: "ssh://me@build.example".to_owned(),
        },
        ..Default::default()
    });
    run.add_cwd(Path::new("/srv/app"));
    let Run::Command(run_command) = run else {
        panic!("add_cwd changed the run variant");
    };
    assert_eq!(run_command.cwd, Some(PathBuf::from("/srv/app/src/project")));
}

#[test]
fn global_cwd_given_to_remote_host_pane_without_cwd() {
    use yinz_types::input::command::RunCommand;
    let mut run = Run::Command(RunCommand {
        command: PathBuf::from("cargo"),
        target: PaneTarget::Ssh {
            host: "ssh://me@build.example".to_owned(),
        },
        ..Default::default()
    });
    run.add_cwd(Path::new("/srv/app"));
    let Run::Command(run_command) = run else {
        panic!("add_cwd changed the run variant");
    };
    assert_eq!(run_command.cwd, Some(PathBuf::from("/srv/app")));
}

fn floating_pane_cwds(floating_panes: &[FloatingPaneLayout]) -> Vec<Option<PathBuf>> {
    floating_panes
        .iter()
        .map(|floating_pane| match &floating_pane.run {
            Some(Run::Cwd(cwd)) => Some(cwd.clone()),
            Some(Run::Command(run_command)) => run_command.cwd.clone(),
            other => panic!("unexpected run for floating pane: {:?}", other),
        })
        .collect()
}

#[test]
fn global_cwd_and_tab_cwd_prepended_to_floating_panes_with_and_without_cwd() {
    let kdl_layout = r#"
        layout {
            cwd "/tmp"
            tab cwd="./foo" {
                pane
                floating_panes {
                    pane // should have /tmp/foo
                    pane cwd="./bar" // should have /tmp/foo/bar
                    pane command="tail" cwd="./bar" // should have /tmp/foo/bar
                }
            }
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    let (_, _, floating_panes) = &layout.tabs[0];
    assert_eq!(
        floating_pane_cwds(floating_panes),
        vec![
            Some(PathBuf::from("/tmp/foo")),
            Some(PathBuf::from("/tmp/foo/bar")),
            Some(PathBuf::from("/tmp/foo/bar")),
        ],
        "the tab cwd composes between the global cwd and the floating pane cwd"
    );
}

#[test]
fn global_cwd_prepended_to_swap_floating_layout_panes() {
    let kdl_layout = r#"
        layout {
            cwd "/tmp"
            pane
            swap_floating_layout {
                floating_panes max_panes=2 {
                    pane // should have /tmp
                    pane cwd="./bar" // should have /tmp/bar
                }
            }
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    let (swap_floating_layout, _name) = &layout.swap_floating_layouts[0];
    let floating_panes = swap_floating_layout
        .get(&LayoutConstraint::MaxPanes(2))
        .expect("the max_panes=2 swap layout");
    assert_eq!(
        floating_pane_cwds(floating_panes),
        vec![Some(PathBuf::from("/tmp")), Some(PathBuf::from("/tmp/bar")),],
        "swap floating layouts have no tab cwd, so they get the global cwd alone"
    );
}

#[test]
fn global_cwd_and_tab_cwd_prepended_to_panes_with_and_without_cwd() {
    let kdl_layout = r#"
        layout {
            cwd "/tmp"
            tab cwd="./foo" {
                pane // should have /tmp/foo
                pane command="tail" cwd="./bar" // should have /tmp/foo/bar
            }
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn global_cwd_and_tab_cwd_prepended_to_panes_with_and_without_cwd_in_pane_templates() {
    let kdl_layout = r#"
        layout {
            cwd "/tmp"
            pane_template name="my_pane_template" {
                pane // should have /tmp/foo
                pane command="tail" cwd="./bar" // should have /tmp/foo/bar
                children
            }
            tab cwd="./foo" {
                my_pane_template {
                    pane // should have /tmp/foo
                }
            }
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn global_cwd_and_tab_cwd_prepended_to_panes_with_and_without_cwd_in_tab_templates() {
    let kdl_layout = r#"
        layout {
            cwd "/tmp"
            tab_template name="my_tab_template" {
                pane // should have /tmp/foo
                pane command="tail" cwd="./bar" // should have /tmp/foo/bar
                children
            }
            my_tab_template cwd="./foo" {
                pane // should have /tmp/foo
            }
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn can_load_swap_layouts_from_a_different_file() {
    let kdl_layout = r#"
        layout {
            // here we define a tab_template in the main layout and later make sure we can sue it
            // in the swap layouts
            tab_template name="ui" {
               pane size=1 borderless=true {
                   plugin location="yinz:tab-bar"
               }
               children
               pane size=2 borderless=true {
                   plugin location="yinz:status-bar"
               }
            }
            pane
        }
    "#;
    let kdl_swap_layout = r#"
        swap_tiled_layout name="vertical" {
            ui max_panes=5 {
                pane split_direction="vertical" {
                    pane
                    pane { children; }
                }
            }
            ui max_panes=8 {
                pane split_direction="vertical" {
                    pane { children; }
                    pane { pane; pane; pane; pane; }
                }
            }
            ui max_panes=12 {
                pane split_direction="vertical" {
                    pane { children; }
                    pane { pane; pane; pane; pane; }
                    pane { pane; pane; pane; pane; }
                }
            }
        }
    "#;
    let layout = crate::kdl::layout::from_kdl(
        kdl_layout,
        Some("layout_file_name".into()),
        Some(("swap_layout_file_name", kdl_swap_layout)),
        None,
    )
    .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn swap_layout_children_with_siblings_preserve_pane_count_and_positions() {
    use yinz_types::pane_size::{PaneGeom, Size};

    for children in [
        r#"children; pane name="after";"#,
        r#"pane name="before"; children; pane name="after";"#,
        r#"pane name="before"; children;"#,
        "children;",
    ] {
        for nested in [false, true] {
            let panes = format!("pane {{ {children} }};");
            let panes = if nested {
                format!("pane {{ {panes} }};")
            } else {
                panes
            };
            let raw_layout = format!("layout {{ swap_tiled_layout {{ tab {{ {panes} }}; }}; }}");
            let layout = crate::kdl::layout::from_kdl(&raw_layout, None, None, None).unwrap();
            let pane_layout = layout.swap_tiled_layouts[0].0.values().next().unwrap();
            let area = PaneGeom::from(&Size {
                cols: 120,
                rows: 40,
            });

            for pane_count in 3..=5 {
                let positions = pane_layout
                    .position_panes_in_space(&area, Some(pane_count), false, true)
                    .unwrap();
                assert_eq!(positions.len(), pane_count, "{raw_layout}");
                if children.contains("before") {
                    assert_eq!(positions[0].0.name.as_deref(), Some("before"));
                }
                if children.contains("after") {
                    assert_eq!(positions.last().unwrap().0.name.as_deref(), Some("after"));
                }
                let mut next_row = 0;
                for (_, geom) in positions {
                    assert_eq!(geom.y, next_row, "{raw_layout}");
                    next_row += geom.rows.as_usize();
                }
                assert_eq!(next_row, area.rows.as_usize(), "{raw_layout}");
            }
        }
    }
}

#[test]
fn swap_layout_expands_only_the_first_children_node() {
    use yinz_types::pane_size::{PaneGeom, Size};

    let raw_layout = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane { pane name="before"; children; }
                    pane { children; }
                }
            }
        }
    "#;
    let layout = crate::kdl::layout::from_kdl(raw_layout, None, None, None).unwrap();
    let pane_layout = layout.swap_tiled_layouts[0].0.values().next().unwrap();
    let area = PaneGeom::from(&Size {
        cols: 120,
        rows: 40,
    });
    let positions = pane_layout
        .position_panes_in_space(&area, Some(4), false, true)
        .unwrap();

    assert_eq!(positions.len(), 4);
    assert_eq!(positions[0].0.name.as_deref(), Some("before"));
    assert_eq!(positions[1].0.external_children_index, Some(0));
}

#[test]
fn bundled_three_column_layouts_are_balanced() {
    use std::path::Path;
    use yinz_types::pane_size::{PaneGeom, Size};

    for (layout_name, ui_pane_count) in [
        ("default", 2),
        ("compact", 1),
        ("classic", 2),
        ("strider", 3),
    ] {
        let (layout_path, raw_layout, raw_swap_layout) =
            crate::layout::stringified_from_default_assets(Path::new(layout_name)).unwrap();
        let (swap_layout_path, raw_swap_layout) = raw_swap_layout.unwrap();
        let layout = crate::kdl::layout::from_kdl(
            &raw_layout,
            Some(layout_path),
            Some((&swap_layout_path, &raw_swap_layout)),
            None,
        )
        .unwrap();
        let three_columns_index = layout
            .swap_tiled_layouts
            .iter()
            .position(|(_, name)| name.as_deref() == Some("three-columns"))
            .unwrap();
        assert_eq!(
            layout.swap_tiled_layouts[three_columns_index - 1]
                .1
                .as_deref(),
            Some("vertical")
        );
        let three_columns = &layout.swap_tiled_layouts[three_columns_index].0;
        let display_area = PaneGeom::from(&Size {
            cols: 120,
            rows: 40,
        });

        for terminal_pane_count in 1..=10 {
            let total_pane_count = terminal_pane_count + ui_pane_count;
            let pane_layout = three_columns
                .iter()
                .find_map(|(constraint, layout)| match constraint {
                    LayoutConstraint::MaxPanes(max_panes) if total_pane_count <= *max_panes => {
                        Some(layout)
                    },
                    _ => None,
                })
                .unwrap();
            let positioned_panes = pane_layout
                .position_panes_in_space(&display_area, Some(total_pane_count), false, false)
                .unwrap();
            let mut columns = BTreeMap::<usize, Vec<usize>>::new();
            for (pane, geom) in positioned_panes {
                if Run::is_terminal(&pane.run) {
                    columns
                        .entry(geom.x)
                        .or_default()
                        .push(geom.cols.as_usize());
                }
            }

            let column_count = terminal_pane_count.min(3);
            assert_eq!(columns.len(), column_count, "{layout_name}");
            let panes_per_column: Vec<_> = columns.values().map(Vec::len).collect();
            let evenly_distributed: Vec<_> = (0..column_count)
                .map(|column| {
                    terminal_pane_count / column_count
                        + usize::from(column < terminal_pane_count % column_count)
                })
                .collect();
            assert_eq!(panes_per_column, evenly_distributed, "{layout_name}");
            let widths: Vec<_> = columns.values().flatten().copied().collect();
            assert!(
                widths.iter().max().unwrap() - widths.iter().min().unwrap() <= 1,
                "{layout_name}"
            );
        }
    }
}

#[test]
fn bundled_quad_layouts_are_balanced() {
    use std::path::Path;
    use yinz_types::pane_size::{PaneGeom, Size};

    for (layout_name, ui_pane_count) in [
        ("default", 2),
        ("compact", 1),
        ("classic", 2),
        ("strider", 3),
    ] {
        let (layout_path, raw_layout, raw_swap_layout) =
            crate::layout::stringified_from_default_assets(Path::new(layout_name)).unwrap();
        let (swap_layout_path, raw_swap_layout) = raw_swap_layout.unwrap();
        let layout = crate::kdl::layout::from_kdl(
            &raw_layout,
            Some(layout_path),
            Some((&swap_layout_path, &raw_swap_layout)),
            None,
        )
        .unwrap();
        let quad_index = layout
            .swap_tiled_layouts
            .iter()
            .position(|(_, name)| name.as_deref() == Some("quad"))
            .unwrap();
        assert_eq!(
            layout.swap_tiled_layouts[quad_index - 1].1.as_deref(),
            Some("three-columns")
        );
        let quad = &layout.swap_tiled_layouts[quad_index].0;

        for (cols, rows) in [(120, 40), (121, 41)] {
            let display_area = PaneGeom::from(&Size { cols, rows });
            let mut content_area = None;
            for terminal_pane_count in 1..=10 {
                let context = format!("{layout_name}: {terminal_pane_count} panes, {cols}x{rows}");
                let total_pane_count = terminal_pane_count + ui_pane_count;
                let pane_layout = quad
                    .iter()
                    .find_map(|(constraint, layout)| match constraint {
                        LayoutConstraint::MaxPanes(max_panes) if total_pane_count <= *max_panes => {
                            Some(layout)
                        },
                        _ => None,
                    })
                    .unwrap();
                let positioned_panes = pane_layout
                    .position_panes_in_space(&display_area, Some(total_pane_count), false, false)
                    .unwrap();
                assert_eq!(positioned_panes.len(), total_pane_count, "{context}");
                let mut columns = BTreeMap::<usize, Vec<PaneGeom>>::new();
                for (pane, geom) in positioned_panes {
                    if Run::is_terminal(&pane.run) {
                        columns.entry(geom.x).or_default().push(geom);
                    }
                }
                let expected_counts = if terminal_pane_count == 1 {
                    vec![1]
                } else {
                    vec![terminal_pane_count / 2, terminal_pane_count.div_ceil(2)]
                };
                assert_eq!(
                    columns.values().map(Vec::len).collect::<Vec<_>>(),
                    expected_counts,
                    "{context}"
                );
                if terminal_pane_count == 1 {
                    content_area = Some(columns.values().next().unwrap()[0]);
                }
                let area = content_area.unwrap();
                let mut next_x = area.x;
                let mut widths = Vec::new();
                for panes in columns.values_mut() {
                    panes.sort_by_key(|geom| geom.y);
                    let width = panes[0].cols.as_usize();
                    widths.push(width);
                    let mut next_y = area.y;
                    // The runtime resizer resolves these proportions after accounting for UI bars.
                    let height_percent = area.rows.as_percent().unwrap() / panes.len() as f64;
                    for geom in panes {
                        assert_eq!(geom.rows.as_percent(), Some(height_percent), "{context}");
                        assert_eq!((geom.x, geom.y), (next_x, next_y), "{context}");
                        assert_eq!(geom.cols.as_usize(), width, "{context}");
                        next_y += geom.rows.as_usize();
                    }
                    assert_eq!(next_y, area.y + area.rows.as_usize(), "{context}");
                    next_x += width;
                }
                assert_eq!(next_x, area.x + area.cols.as_usize(), "{context}");
                assert!(
                    widths.iter().max().unwrap() - widths.iter().min().unwrap() <= 1,
                    "{context}"
                );
            }
        }
    }
}

#[test]
fn can_define_stacked_children_for_pane_node() {
    let kdl_layout = r#"
        layout {
           pane stacked=true {
               pane
               pane
           }
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn can_define_stacked_children_for_pane_template() {
    let kdl_layout = r#"
        layout {
           pane_template name="stack" stacked=true {
               children
           }
           stack {
               pane
               pane
           }
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn can_define_a_stack_with_an_expanded_pane() {
    let kdl_layout = r#"
        layout {
           pane stacked=true {
               pane
               pane expanded=true
               pane
           }
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{:#?}", layout)));
}

#[test]
fn cannot_define_stacked_panes_for_bare_node() {
    let kdl_layout = r#"
        layout {
           pane stacked=true
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None);
    assert!(layout.is_err(), "error provided for tab name with space");
}

#[test]
fn cannot_define_an_expanded_pane_outside_of_a_stack() {
    let kdl_layout = r#"
        layout {
            pane {
                pane
                pane expanded=true
            }
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None);
    assert!(layout.is_err(), "error provided for tab name with space");
}

#[test]
fn cannot_define_stacked_panes_with_vertical_split_direction() {
    let kdl_layout = r#"
        layout {
           pane stacked=true split_direction="vertical" {
               pane
               pane
           }
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None);
    assert!(layout.is_err(), "error provided for tab name with space");
}

#[test]
fn cannot_define_stacked_panes_with_grandchildren() {
    let kdl_layout = r#"
        layout {
           pane stacked=true {
               pane {
                   pane
                   pane
               }
               pane
           }
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None);
    assert!(layout.is_err(), "error provided for tab name with space");
}

#[test]
fn cannot_define_stacked_panes_with_grandchildren_in_pane_template() {
    let kdl_layout = r#"
        layout {
           pane_template name="stack" stacked=true {
               children
           }
           stack {
               pane
               pane {
                   pane
                   pane
               }
           }
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None);
    assert!(layout.is_err(), "error provided for tab name with space");
}

#[test]
fn run_plugin_location_parsing() {
    let kdl_layout = r#"
        layout {
            pane {
                plugin location="yinz:tab-bar"
            }
            pane {
                plugin location="file:/path/to/my/plugin.scm"
            }
            pane {
                plugin location="file:plugin.scm"
            }
            pane {
                plugin location="file:relative/with space/plugin.scm"
            }
            pane {
                plugin location="file:///absolute/with space/plugin.scm"
            }
            pane {
                plugin location="filepicker"
            }
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();
    let expected_layout = Layout {
        template: Some((
            TiledPaneLayout {
                children: vec![
                    TiledPaneLayout {
                        run: Some(Run::Plugin(RunPluginOrAlias::RunPlugin(RunPlugin {
                            _allow_exec_host_cmd: false,
                            location: RunPluginLocation::Yinz(PluginTag::new("tab-bar")),
                            configuration: Default::default(),
                            ..Default::default()
                        }))),
                        ..Default::default()
                    },
                    TiledPaneLayout {
                        run: Some(Run::Plugin(RunPluginOrAlias::RunPlugin(RunPlugin {
                            _allow_exec_host_cmd: false,
                            location: RunPluginLocation::File(PathBuf::from(
                                "/path/to/my/plugin.scm",
                            )),
                            configuration: Default::default(),
                            ..Default::default()
                        }))),
                        ..Default::default()
                    },
                    TiledPaneLayout {
                        run: Some(Run::Plugin(RunPluginOrAlias::RunPlugin(RunPlugin {
                            _allow_exec_host_cmd: false,
                            location: RunPluginLocation::File(
                                std::env::current_dir().unwrap().join("plugin.scm"),
                            ),
                            configuration: Default::default(),
                            ..Default::default()
                        }))),
                        ..Default::default()
                    },
                    TiledPaneLayout {
                        run: Some(Run::Plugin(RunPluginOrAlias::RunPlugin(RunPlugin {
                            _allow_exec_host_cmd: false,
                            location: RunPluginLocation::File(
                                std::env::current_dir()
                                    .unwrap()
                                    .join("relative/with space/plugin.scm"),
                            ),
                            configuration: Default::default(),
                            ..Default::default()
                        }))),
                        ..Default::default()
                    },
                    TiledPaneLayout {
                        run: Some(Run::Plugin(RunPluginOrAlias::RunPlugin(RunPlugin {
                            _allow_exec_host_cmd: false,
                            location: RunPluginLocation::File(PathBuf::from(
                                "/absolute/with space/plugin.scm",
                            )),
                            configuration: Default::default(),
                            ..Default::default()
                        }))),
                        ..Default::default()
                    },
                    TiledPaneLayout {
                        run: Some(Run::Plugin(RunPluginOrAlias::Alias(PluginAlias {
                            name: "filepicker".to_owned(),
                            configuration: Some(PluginUserConfiguration::default()),
                            ..Default::default()
                        }))),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
            vec![],
        )),
        ..Default::default()
    };
    assert_eq!(layout, expected_layout);
}

#[cfg(unix)]
#[test]
fn env_var_expansion() {
    let raw_layout = r#"
        layout {
            // cwd tests + composition
            cwd "$TEST_GLOBAL_CWD"
            pane cwd="relative"  // -> /abs/path/relative
            pane cwd="/another/abs"  // -> /another/abs
            pane cwd="$TEST_LOCAL_CWD"  // -> /another/abs
            pane cwd="$TEST_RELATIVE"  // -> /abs/path/relative
            pane command="ls" cwd="$TEST_ABSOLUTE"  // -> /somewhere
            pane edit="file.rs" cwd="$TEST_ABSOLUTE"  // -> /somewhere/file.rs
            pane edit="file.rs" cwd="~/backup"  // -> /home/aram/backup/file.rs

            // other paths
            pane command="~/backup/executable"  // -> /home/aram/backup/executable
            pane edit="~/backup/foo.txt"  // -> /home/aram/backup/foo.txt
        }
    "#;
    let env_vars = [
        ("TEST_GLOBAL_CWD", "/abs/path"),
        ("TEST_LOCAL_CWD", "/another/abs"),
        ("TEST_RELATIVE", "relative"),
        ("TEST_ABSOLUTE", "/somewhere"),
        ("HOME", "/home/aram"),
    ];
    let mut old_vars = Vec::new();
    // set environment variables for test, keeping track of existing values.
    for (key, value) in env_vars {
        old_vars.push((key, std::env::var(key).ok()));
        // FIXME: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::set_var(key, value) };
    }
    let layout =
        crate::kdl::layout::from_kdl(raw_layout, Some("layout_file_name".into()), None, None);
    // restore environment.
    for (key, opt) in old_vars {
        match opt {
            // FIXME: Audit that the environment access only happens in single-threaded code.
            Some(value) => unsafe { std::env::set_var(key, &value) },
            // FIXME: Audit that the environment access only happens in single-threaded code.
            None => unsafe { std::env::remove_var(key) },
        }
    }
    let layout = layout.unwrap();
    assert_snapshot!(normalize_layout_debug(format!("{layout:#?}")));
}

#[test]
fn env_var_missing() {
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::remove_var("SOME_UNIQUE_VALUE") };
    let kdl_layout = r#"
        layout {
            cwd "$SOME_UNIQUE_VALUE"
            pane cwd="relative"
        }
    "#;
    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None);
    assert!(layout.is_err(), "invalid env var lookup should fail");
}

#[test]
fn layout_node_with_tab_name() {
    let kdl_layout = r#"
        layout name="my-layout" {
            pane
        }
    "#;

    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();

    assert_eq!(layout.tabs.len(), 1);
    assert_eq!(layout.tabs[0].0, Some("my-layout".to_string()));
}

#[test]
fn layout_node_with_split_direction() {
    let kdl_layout = r#"
        layout split_direction="vertical" {
            pane
            pane
        }
    "#;

    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();

    assert_eq!(
        layout.tabs[0].1.children_split_direction,
        SplitDirection::Vertical
    );
}

#[test]
fn layout_node_with_hide_floating_panes() {
    let kdl_layout = r#"
        layout hide_floating_panes=true {
            pane
        }
    "#;

    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();

    assert!(layout.tabs[0].1.hide_floating_panes);
}

#[test]
fn layout_node_with_cwd() {
    let kdl_layout = r#"
        layout cwd="/tmp" {
            pane
        }
    "#;

    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();

    // Verify cwd was applied - check the first pane's run property
    assert!(!layout.tabs.is_empty());
    // The cwd should be propagated to children
    if let Some(Run::Cwd(path)) = &layout.tabs[0].1.children[0].run {
        assert_eq!(path.to_str(), Some("/tmp"));
    } else {
        panic!("Expected cwd to be set on pane");
    }
}

#[test]
fn layout_node_with_multiple_tab_properties() {
    let kdl_layout = r#"
        layout name="test" split_direction="vertical" hide_floating_panes=true {
            pane
            pane
        }
    "#;

    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();

    assert_eq!(layout.tabs[0].0, Some("test".to_string()));
    assert_eq!(
        layout.tabs[0].1.children_split_direction,
        SplitDirection::Vertical
    );
    assert!(layout.tabs[0].1.hide_floating_panes);
}

#[test]
fn layout_node_with_name_and_explicit_tabs_errors() {
    let kdl_layout = r#"
        layout name="test" {
            tab {
                pane
            }
        }
    "#;

    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None);

    assert!(layout.is_err());
    let error_message = format!("{:?}", layout.unwrap_err());
    assert!(error_message.contains(
        "name' property on the layout node is treated as a tab name and cannot be used when there are explicit tab nodes"
    ));
}

#[test]
fn layout_node_with_split_direction_and_explicit_tabs_errors() {
    let kdl_layout = r#"
        layout split_direction="vertical" {
            tab {
                pane
            }
        }
    "#;

    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None);

    assert!(layout.is_err());
    let error_message = format!("{:?}", layout.unwrap_err());
    assert!(error_message.contains(
        "split_direction' property on the layout node is treated as a tab property and cannot be used when there are explicit tab nodes"
    ));
}

#[test]
fn layout_node_with_hide_floating_panes_and_explicit_tabs_errors() {
    let kdl_layout = r#"
        layout hide_floating_panes=true {
            tab {
                pane
            }
        }
    "#;

    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None);

    assert!(layout.is_err());
    let error_message = format!("{:?}", layout.unwrap_err());
    assert!(error_message.contains(
        "hide_floating_panes' property on the layout node is treated as a tab property and cannot be used when there are explicit tab nodes"
    ));
}

#[test]
fn layout_node_with_name_and_only_floating_panes() {
    let kdl_layout = r#"
        layout name="floating-only" {
            floating_panes {
                pane
            }
        }
    "#;

    let layout =
        crate::kdl::layout::from_kdl(kdl_layout, Some("layout_file_name".into()), None, None)
            .unwrap();

    // Verify the tab name was applied
    assert_eq!(layout.tabs.len(), 1);
    assert_eq!(layout.tabs[0].0, Some("floating-only".to_string()));
}

#[test]
fn floating_pane_coordinates_allows_zero_percent_for_x_y() {
    use yinz_types::geometry::FloatingPaneCoordinates;
    use yinz_types::geometry::PercentOrFixed;

    let coords = FloatingPaneCoordinates::new(
        Some("0%".to_string()),
        Some("0%".to_string()),
        Some("50%".to_string()),
        Some("50%".to_string()),
        None,
        Some(false),
    );

    assert!(coords.is_some());
    let coords = coords.unwrap();
    assert_eq!(coords.x, Some(PercentOrFixed::Percent(0)));
    assert_eq!(coords.y, Some(PercentOrFixed::Percent(0)));
    assert_eq!(coords.width, Some(PercentOrFixed::Percent(50)));
    assert_eq!(coords.height, Some(PercentOrFixed::Percent(50)));
}

#[test]
fn floating_pane_coordinates_rejects_zero_percent_for_width() {
    use yinz_types::geometry::FloatingPaneCoordinates;
    use yinz_types::geometry::PercentOrFixed;

    let coords = FloatingPaneCoordinates::new(
        Some("10%".to_string()),
        Some("10%".to_string()),
        Some("0%".to_string()),
        Some("50%".to_string()),
        None,
        Some(false),
    );

    // Should have coords but width will be None due to rejection
    assert!(coords.is_some());
    let coords = coords.unwrap();
    assert!(coords.width.is_none());
    assert_eq!(coords.height, Some(PercentOrFixed::Percent(50)));
}

#[test]
fn floating_pane_coordinates_rejects_zero_percent_for_height() {
    use yinz_types::geometry::FloatingPaneCoordinates;
    use yinz_types::geometry::PercentOrFixed;

    let coords = FloatingPaneCoordinates::new(
        Some("10%".to_string()),
        Some("10%".to_string()),
        Some("50%".to_string()),
        Some("0%".to_string()),
        None,
        Some(false),
    );

    // Should have coords but height will be None due to rejection
    assert!(coords.is_some());
    let coords = coords.unwrap();
    assert_eq!(coords.width, Some(PercentOrFixed::Percent(50)));
    assert!(coords.height.is_none());
}

#[test]
fn floating_pane_coordinates_static_zero_works() {
    use yinz_types::geometry::FloatingPaneCoordinates;
    use yinz_types::geometry::PercentOrFixed;

    let coords = FloatingPaneCoordinates::new(
        Some("0".to_string()),
        Some("0".to_string()),
        Some("100".to_string()),
        Some("100".to_string()),
        None,
        Some(false),
    );

    assert!(coords.is_some());
    let coords = coords.unwrap();
    assert_eq!(coords.x, Some(PercentOrFixed::Fixed(0)));
    assert_eq!(coords.y, Some(PercentOrFixed::Fixed(0)));
    assert_eq!(coords.width, Some(PercentOrFixed::Fixed(100)));
    assert_eq!(coords.height, Some(PercentOrFixed::Fixed(100)));
}

#[test]
fn floating_pane_coordinates_one_percent_works() {
    use yinz_types::geometry::FloatingPaneCoordinates;
    use yinz_types::geometry::PercentOrFixed;

    let coords = FloatingPaneCoordinates::new(
        Some("1%".to_string()),
        Some("1%".to_string()),
        Some("50%".to_string()),
        Some("50%".to_string()),
        None,
        Some(false),
    );

    assert!(coords.is_some());
    let coords = coords.unwrap();
    assert_eq!(coords.x, Some(PercentOrFixed::Percent(1)));
    assert_eq!(coords.y, Some(PercentOrFixed::Percent(1)));
    assert_eq!(coords.width, Some(PercentOrFixed::Percent(50)));
    assert_eq!(coords.height, Some(PercentOrFixed::Percent(50)));
}

#[test]
fn tiled_pane_still_rejects_zero_percent() {
    use std::str::FromStr;
    use yinz_types::geometry::SplitSize;

    // Verify SplitSize::from_str still rejects 0%
    let result = SplitSize::from_str("0%");
    assert!(result.is_err());

    // But 1% should work
    let result = SplitSize::from_str("1%");
    assert!(result.is_ok());
}

#[test]
fn plugin_alias_records_the_caller_terminal_without_overwriting_it() {
    let mut alias = PluginAlias::new("plugin", &None, None);
    alias
        .set_caller_terminal_pane_id_if_not_set(Some(yinz_types::internal_ids::TerminalId::new(7)));
    alias
        .set_caller_terminal_pane_id_if_not_set(Some(yinz_types::internal_ids::TerminalId::new(9)));

    assert_eq!(
        alias
            .configuration
            .unwrap()
            .inner()
            .get("caller_terminal_pane_id")
            .map(String::as_str),
        Some("7")
    );
}

#[test]
fn caller_context_does_not_change_plugin_alias_identity() {
    let configuration = Some(std::collections::BTreeMap::from([(
        "duplicate_focused".to_owned(),
        "true".to_owned(),
    )]));
    let requested =
        RunPluginOrAlias::Alias(PluginAlias::new("duplicate-agent", &configuration, None));
    let mut running = PluginAlias::new("duplicate-agent", &configuration, None);
    running.set_caller_cwd_if_not_set(Some("/repo".into()));
    running
        .set_caller_terminal_pane_id_if_not_set(Some(yinz_types::internal_ids::TerminalId::new(7)));

    assert!(requested.is_equivalent_to_run(&Some(Run::Plugin(RunPluginOrAlias::Alias(running)))));
}

/// Which URLs have a relative form to remember, and which have none.
///
/// The distinction is the whole of how the user-plugin-folder fallback stays honest: a name is
/// looked for in the folder, a path the user spelled out in full is not.
#[test]
fn only_a_relative_file_url_reports_the_path_as_written() {
    let cwd = std::path::PathBuf::from("/some/where");

    let (location, relative) = yinz_runtime::plugin_location::parse_plugin_location_with_relative(
        "file:keys.scm",
        Some(cwd.clone()),
    )
    .unwrap();
    assert_eq!(relative, Some(std::path::PathBuf::from("keys.scm")));
    assert_eq!(
        location,
        RunPluginLocation::File(cwd.join("keys.scm")),
        "the location itself is still the absolute path everything downstream expects"
    );

    for absolute in ["file:/etc/keys.scm", "file:~/keys.scm"] {
        let (_, relative) = yinz_runtime::plugin_location::parse_plugin_location_with_relative(
            absolute,
            Some(cwd.clone()),
        )
        .unwrap();
        assert_eq!(relative, None, "{absolute} was given in full");
    }

    let (_, relative) = yinz_runtime::plugin_location::parse_plugin_location_with_relative(
        "yinz:tab-bar",
        Some(cwd),
    )
    .unwrap();
    assert_eq!(relative, None, "a builtin is named by a tag, not a path");
}

#[test]
fn a_bubblewrap_pane_keeps_its_command_and_names_its_checkout() {
    let layout = crate::kdl::layout::from_kdl(
        r#"layout { pane command="claude" cwd="/repo/.worktrees/task" bubblewrap="/repo" { args "--resume"; }; }"#,
        Some("layout_file_name".into()),
        None,
        None,
    )
    .unwrap();
    let Some(Run::Command(command)) = &layout.template.as_ref().unwrap().0.children[0].run else {
        panic!("expected a command pane");
    };
    assert_eq!(command.command, PathBuf::from("claude"));
    assert_eq!(command.args, ["--resume"]);
    assert_eq!(
        command.target,
        PaneTarget::Bubblewrap {
            root: PathBuf::from("/repo")
        }
    );
    for (layout, why) in [
        (
            r#"layout { pane command="claude" bubblewrap="relative/repo" }"#,
            "a relative checkout",
        ),
        (
            r#"layout { pane bubblewrap="/repo" }"#,
            "a sandbox with no command",
        ),
        (
            r#"layout { pane edit="notes.md" bubblewrap="/repo" }"#,
            "an edit pane in a sandbox",
        ),
    ] {
        assert!(
            crate::kdl::layout::from_kdl(layout, Some("layout_file_name".into()), None, None)
                .is_err(),
            "{why} should be a layout error"
        );
    }
}

#[test]
fn pane_tabs_layout_validates_flat_named_plugins_and_selects_one_run() {
    for invalid in [
        "pane tabbed=true",
        "pane selected=0",
        "floating_panes { pane { pane name=\"Files\" { plugin location=\"yinz:strider\"; }; }; }",
        "pane tabbed=true { pane name=\"shell\"; }",
        "pane tabbed=true { pane { plugin location=\"yinz:strider\"; }; }",
        "pane tabbed=true selected=2 { pane name=\"Files\" { plugin location=\"yinz:strider\"; }; }",
        "pane tabbed=true { pane name=\"nested\" tabbed=true { pane name=\"Files\" { plugin location=\"yinz:strider\"; }; }; }",
    ] {
        let raw = format!("layout {{ {invalid}; }}");
        assert!(crate::kdl::layout::from_kdl(&raw, None, None, None).is_err(), "{raw}");
    }
    for prefix in ["", "floating_panes {"] {
        let suffix = if prefix.is_empty() { "" } else { "};" };
        let raw = format!(
            r#"layout {{ {prefix}
            pane tabbed=true selected=1 {{
                pane name="Files" {{ plugin location="yinz:strider"; }}
                pane name="Files" {{ plugin location="yinz:strider"; }}
            }}; {suffix}
        }}"#
        );
        assert!(
            crate::kdl::layout::from_kdl(&raw, None, None, None).is_ok(),
            "{raw}"
        );
    }
}

#[test]
fn pane_tabs_templates_merge_selection_and_keep_child_cwds() {
    let raw = r#"layout cwd="/project" {
        pane_template name="views" tabbed=true selected=1 {
            pane name="Files" { plugin location="strider" cwd="files"; }
            pane name="Files" { plugin location="yinz:strider" cwd="files"; }
        }
        views
        views selected=0
    }"#;
    let layout = crate::kdl::layout::from_kdl(raw, None, None, None).unwrap();
    let (tiled, _) = layout.template.unwrap();
    for (pane, selected) in tiled.children.iter().zip([1, 0]) {
        let tabs = pane.pane_tabs.as_ref().unwrap();
        assert_eq!(tabs.active, selected);
        assert_eq!(tabs.tabs[0].location, "strider");
        assert_eq!(
            tabs.tabs[0].initial_cwd,
            Some(PathBuf::from("/project/files"))
        );
        assert_eq!(
            tabs.tabs[1].initial_cwd,
            Some(PathBuf::from("/project/files"))
        );
        let Run::Plugin(plugin) = pane.run.as_ref().unwrap() else {
            panic!("selected plugin")
        };
        assert_eq!(plugin.get_initial_cwd(), tabs.tabs[selected].initial_cwd);
        assert!(pane.children.is_empty());
    }
    for invalid in ["selected=2", "tabbed=false", "selected=-1"] {
        let raw = format!("layout {{ pane_template name=\"views\" tabbed=true selected=0 {{ pane name=\"Files\" {{ plugin location=\"yinz:strider\"; }} }} views {invalid}; }}");
        assert!(
            crate::kdl::layout::from_kdl(&raw, None, None, None).is_err(),
            "{raw}"
        );
    }
    for invalid_child in [
        "pane name=\"Command\" command=\"echo\"",
        "pane name=\"Stack\" stacked=true { pane name=\"Child\" { plugin location=\"yinz:strider\"; } }",
        "pane name=\"Nested\" tabbed=true { pane name=\"Child\" { plugin location=\"yinz:strider\"; } }",
    ] {
        let raw = format!("layout {{ pane_template name=\"views\" tabbed=true {{ {invalid_child}; }} views; }}");
        assert!(crate::kdl::layout::from_kdl(&raw, None, None, None).is_err(), "{raw}");
    }
}

#[test]
fn floating_pane_tabs_keep_their_child_cwds() {
    let raw = r#"layout cwd="/project" {
        floating_panes {
            pane tabbed=true selected=1 {
                pane name="Files" { plugin location="yinz:strider" cwd="files"; }
                pane name="Files" { plugin location="yinz:strider" cwd="files"; }
            }
        }
    }"#;
    let layout = crate::kdl::layout::from_kdl(raw, None, None, None).unwrap();
    let (_, floating) = layout.template.unwrap();
    let pane = &floating[0];
    let tabs = pane.pane_tabs.as_ref().unwrap();
    assert_eq!(
        tabs.tabs[0].initial_cwd,
        Some(PathBuf::from("/project/files"))
    );
    assert_eq!(
        tabs.tabs[1].initial_cwd,
        Some(PathBuf::from("/project/files"))
    );
    let Run::Plugin(plugin) = pane.run.as_ref().unwrap() else {
        panic!("selected plugin")
    };
    assert_eq!(plugin.get_initial_cwd(), tabs.tabs[1].initial_cwd);
}

#[test]
#[cfg(unix)]
fn lazy_pane_tabs_preserve_plugin_directory_fallback_and_symlink_declarations() {
    let cwd = tempfile::tempdir().unwrap();
    let plugins = tempfile::tempdir().unwrap();
    let source = "(void)";
    let path = plugins.path().join("inactive.scm");
    std::fs::write(&path, source).unwrap();
    let link = cwd.path().join("current # %.scm");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    for location in [
        "file:inactive.scm".to_owned(),
        url::Url::from_file_path(&link).unwrap().to_string(),
    ] {
        for floating in [false, true] {
            let pane = format!(
                r#"pane tabbed=true {{
                pane name="Active" {{ plugin location="yinz:strider"; }}
                pane name="Inactive" {{ plugin location="{location}"; }}
            }}"#
            );
            let pane = if floating {
                format!("floating_panes {{ {pane}; }}")
            } else {
                pane
            };
            let raw = format!("layout cwd=\"{}\" {{ {pane}; }}", cwd.path().display());
            let layout = crate::kdl::layout::from_kdl(&raw, None, None, None).unwrap();
            let (tiled, floats) = layout.template.unwrap();
            let tabs = if floating {
                floats[0].pane_tabs.as_ref().unwrap()
            } else {
                tiled.children[0].pane_tabs.as_ref().unwrap()
            };
            assert_eq!(tabs.active, 0);
            let descriptor = &tabs.tabs[1];
            assert_eq!(descriptor.location, location);
            let plugin =
                yinz_runtime::plugin_location::run_plugin_from_url(&descriptor.location).unwrap();
            let config = crate::plugins::PluginConfig::from_run_plugin(&plugin).unwrap();
            let (resolved, actual) = config.resolve_source(Some(plugins.path())).unwrap();
            assert_eq!(
                resolved.canonicalize().unwrap(),
                path.canonicalize().unwrap()
            );
            assert_eq!(actual, source);
        }
    }
}

#[test]
fn remote_layouts_are_refused() {
    let url = "https://example.com/layout.kdl";
    let refused = |result: Result<StringifiedLayout, crate::config::ConfigError>| match result {
        Err(crate::config::ConfigError::RemoteLayoutUnsupported(named)) => {
            assert_eq!(named, url);
        },
        other => panic!("expected RemoteLayoutUnsupported, got {other:?}"),
    };
    refused(stringified_from_path_or_default(
        Some(&PathBuf::from(url)),
        None,
    ));
    // The CLI joins a url onto its cwd before it reaches a loader.
    let cwd = PathBuf::from("/some/cwd");
    let Some(LayoutInfo::File(joined, _)) =
        layout_info_from_cli(&None, &Some(PathBuf::from(url)), cwd)
    else {
        panic!("a url is resolved as a file path")
    };
    refused(stringified_from_dir(&PathBuf::from(&joined), None));
    refused(stringified_from_path_or_default(
        Some(&PathBuf::from(&joined)),
        None,
    ));
    let error = from_layout_info(&None, LayoutInfo::File(joined, Default::default()))
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("remote layouts are not supported"),
        "{error}"
    );
    // A local path that merely mentions http is left alone.
    assert!(!matches!(
        stringified_from_path(Path::new("/tmp/http-layouts/missing.kdl")),
        Err(crate::config::ConfigError::RemoteLayoutUnsupported(_))
    ));
}

#[test]
fn bubblewrap_templates_resolve_before_validation() {
    for (template, consumer, command, root) in [
        (
            r#"command="claude""#,
            r#"bubblewrap="/repo""#,
            "claude",
            "/repo",
        ),
        (
            r#"bubblewrap="/repo""#,
            r#"command="claude""#,
            "claude",
            "/repo",
        ),
        (
            r#"command="claude" bubblewrap="/repo""#,
            r#"command="codex" bubblewrap="/other""#,
            "codex",
            "/other",
        ),
    ] {
        for floating in [false, true] {
            let pane = format!("sandbox {consumer}");
            let pane = if floating {
                format!("floating_panes {{ {pane}; }}")
            } else {
                pane
            };
            let source = format!("layout {{ pane_template name=\"sandbox\" {template}; {pane}; }}");
            let layout = crate::kdl::layout::from_kdl(&source, None, None, None).unwrap();
            let (tiled, floats) = layout.template.unwrap();
            let run = if floating {
                &floats[0].run
            } else {
                &tiled.children[0].run
            };
            let Some(Run::Command(run)) = run else {
                panic!("expected command")
            };
            assert_eq!(run.command, PathBuf::from(command));
            assert_eq!(
                run.target,
                PaneTarget::bubblewrap(std::path::Path::new(root)).unwrap()
            );
            assert!(!run.remote_default_shell);
        }
    }
    for pane in [
        "sandbox",
        "floating_panes { sandbox; }",
        "swap_tiled_layout { tab { sandbox; }; }",
        "swap_floating_layout { floating_panes { sandbox; }; }",
        r#"sandbox edit="notes.md""#,
        r#"sandbox { plugin location="yinz:strider"; }"#,
    ] {
        let source =
            format!("layout {{ pane_template name=\"sandbox\" bubblewrap=\"/repo\"; {pane}; }}");
        assert!(
            crate::kdl::layout::from_kdl(&source, None, None, None).is_err(),
            "{source}"
        );
    }
    assert!(crate::kdl::layout::from_kdl(
        r#"layout { pane_template name="sandbox" bubblewrap="/repo"; pane; }"#,
        None,
        None,
        None
    )
    .is_ok());
}

#[test]
fn bubblewrap_swap_templates_are_validated_after_merging() {
    for swap in [
        "swap_tiled_layout { tab { sandbox COMMAND; }; }",
        "swap_floating_layout { floating_panes { sandbox COMMAND; }; }",
    ] {
        for command in ["", r#"command="claude""#] {
            let swap = swap.replace("COMMAND", command);
            let source =
                format!(r#"layout {{ pane_template name="sandbox" bubblewrap="/repo"; {swap}; }}"#);
            assert_eq!(
                crate::kdl::layout::from_kdl(&source, None, None, None).is_ok(),
                !command.is_empty(),
                "{source}"
            );
            let source = r#"layout { pane_template name="sandbox" bubblewrap="/repo"; pane; }"#;
            assert_eq!(
                crate::kdl::layout::from_kdl(source, None, Some(("swaps.kdl", &swap)), None)
                    .is_ok(),
                !command.is_empty(),
                "{swap}"
            );
        }
    }
}

#[test]
fn bubblewrap_template_arguments_and_explicit_flags_survive_merging() {
    for (template, consumer, args) in [
        (
            r#"bubblewrap="/repo" { args "--resume"; }"#,
            r#"command="claude""#,
            vec!["--resume"],
        ),
        (
            r#"command="claude" { args "--resume"; }"#,
            r#"bubblewrap="/repo" { args "--new"; }"#,
            vec!["--new"],
        ),
    ] {
        let source = format!(
            r#"layout {{ pane_template name="sandbox" {template}; sandbox start_suspended=true close_on_exit=true {consumer}; }}"#
        );
        let layout = crate::kdl::layout::from_kdl(&source, None, None, None).unwrap();
        let Some(Run::Command(command)) = &layout.template.as_ref().unwrap().0.children[0].run
        else {
            panic!("expected command")
        };
        assert_eq!(command.args, args);
        assert!(command.hold_on_start);
        assert!(!command.hold_on_close);
    }
}
