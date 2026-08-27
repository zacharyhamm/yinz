use super::*;

#[test]
fn launch_plugin_global_placement_kdl_round_trip() {
    let source = r#"
        keybinds {
            normal {
                bind "Ctrl g" {
                    LaunchOrFocusPlugin "hocus" {
                        global true
                        edge "left"
                        size "25%"
                    }
                }
            }
        }
    "#;
    let document: KdlDocument = source.parse().unwrap();
    let keybinds = Keybinds::from_kdl(
        document.get("keybinds").unwrap(),
        Default::default(),
        &Default::default(),
    )
    .unwrap();
    let key = KeyWithModifier::new(BareKey::Char('g')).with_ctrl_modifier();
    assert!(matches!(
        keybinds
            .get_actions_for_key_in_mode(&InputMode::Normal, &key)
            .and_then(|actions| actions.first()),
        Some(Action::LaunchOrFocusPlugin {
            global_placement: Some(GlobalPanePlacement {
                edge: GlobalPaneEdge::Left,
                size: PercentOrFixed::Percent(25),
                length: None
            }),
            ..
        })
    ));

    let serialized = Keybinds::to_kdl(&keybinds, true);
    let serialized = serialized.to_string();
    assert!(serialized.contains("global true"));
    assert!(serialized.contains("edge \"left\""));
    assert!(serialized.contains("size \"25%\""));
    let reparsed = Keybinds::from_kdl(
        serialized
            .parse::<KdlDocument>()
            .unwrap()
            .get("keybinds")
            .unwrap(),
        Default::default(),
        &Default::default(),
    )
    .unwrap();
    assert_eq!(keybinds, reparsed);
}

#[test]
fn global_plugin_placement_rejects_move_to_focused_tab() {
    let source = r#"
        keybinds {
            normal {
                bind "Ctrl g" {
                    LaunchOrFocusPlugin "hocus" {
                        global true
                        edge "left"
                        size "25%"
                        move_to_focused_tab true
                    }
                }
            }
        }
    "#;
    let document: KdlDocument = source.parse().unwrap();
    assert!(Keybinds::from_kdl(
        document.get("keybinds").unwrap(),
        Default::default(),
        &Default::default(),
    )
    .is_err());
}

#[test]
fn global_plugin_placement_rejects_overlays() {
    for action in ["LaunchOrFocusPlugin", "LaunchPlugin"] {
        for overlay_setting in [
            "overlay true",
            "overlay \"true\"",
            "modal true",
            "modal \"true\"",
        ] {
            let source = format!(
                r#"
                keybinds {{
                    normal {{
                        bind "Ctrl g" {{
                            {action} "hocus" {{
                                global true
                                edge "left"
                                size "25%"
                                {overlay_setting}
                            }}
                        }}
                    }}
                }}
            "#
            );
            let document: KdlDocument = source.parse().unwrap();
            assert!(Keybinds::from_kdl(
                document.get("keybinds").unwrap(),
                Default::default(),
                &Default::default(),
            )
            .is_err());
        }
    }
}

#[test]
fn launch_or_focus_plugin_preserves_overlay_and_modal_configuration() {
    let source = r#"
        keybinds {
            normal {
                bind "Ctrl o" {
                    LaunchOrFocusPlugin "hocus-ask" {
                        floating true
                        overlay true
                        modal true
                    }
                }
            }
        }
    "#;
    let document: KdlDocument = source.parse().unwrap();
    let keybinds = Keybinds::from_kdl(
        document.get("keybinds").unwrap(),
        Default::default(),
        &Default::default(),
    )
    .unwrap();
    let key = KeyWithModifier::new(BareKey::Char('o')).with_ctrl_modifier();
    let Some(Action::LaunchOrFocusPlugin { plugin, .. }) = keybinds
        .get_actions_for_key_in_mode(&InputMode::Normal, &key)
        .and_then(|actions| actions.first())
    else {
        panic!("expected LaunchOrFocusPlugin");
    };

    let configuration = match plugin {
        RunPluginOrAlias::RunPlugin(plugin) => Some(&plugin.configuration),
        RunPluginOrAlias::Alias(alias) => alias.configuration.as_ref(),
    };
    assert!(configuration.is_some_and(|configuration| {
        ["overlay", "modal"].into_iter().all(|key| {
            configuration
                .inner()
                .get(key)
                .is_some_and(|value| value == "true")
        })
    }));
    let serialized = Keybinds::to_kdl(&keybinds, true).to_string();
    assert!(serialized.contains("overlay true"));
    assert!(serialized.contains("modal true"));
    let reparsed = Keybinds::from_kdl(
        serialized
            .parse::<KdlDocument>()
            .unwrap()
            .get("keybinds")
            .unwrap(),
        Default::default(),
        &Default::default(),
    )
    .unwrap();
    assert_eq!(keybinds, reparsed);
}

#[test]
fn non_global_plugin_placement_named_configuration_round_trips() {
    let source = r#"
        keybinds {
            normal {
                bind "Ctrl g" {
                    LaunchOrFocusPlugin "zellij:strider" {
                        global false
                        edge "soft"
                        size "large"
                    }
                }
            }
        }
    "#;
    let document: KdlDocument = source.parse().unwrap();
    let keybinds = Keybinds::from_kdl(
        document.get("keybinds").unwrap(),
        Default::default(),
        &Default::default(),
    )
    .unwrap();
    let key = KeyWithModifier::new(BareKey::Char('g')).with_ctrl_modifier();
    let action = keybinds
        .get_actions_for_key_in_mode(&InputMode::Normal, &key)
        .and_then(|actions| actions.first())
        .unwrap();
    let Action::LaunchOrFocusPlugin {
        plugin,
        global_placement,
        ..
    } = action
    else {
        panic!("expected LaunchOrFocusPlugin");
    };
    assert_eq!(*global_placement, None);
    let configuration = plugin.get_configuration().unwrap();
    assert_eq!(
        configuration.inner().get("global").map(String::as_str),
        Some("false")
    );
    assert_eq!(
        configuration.inner().get("edge").map(String::as_str),
        Some("soft")
    );
    assert_eq!(
        configuration.inner().get("size").map(String::as_str),
        Some("large")
    );

    let serialized = Keybinds::to_kdl(&keybinds, true).to_string();
    assert!(serialized.contains("global \"false\""));
    assert!(serialized.contains("edge \"soft\""));
    assert!(serialized.contains("size \"large\""));
    let reparsed = Keybinds::from_kdl(
        serialized
            .parse::<KdlDocument>()
            .unwrap()
            .get("keybinds")
            .unwrap(),
        Default::default(),
        &Default::default(),
    )
    .unwrap();
    assert_eq!(keybinds, reparsed);
}

#[test]
fn terminal_global_placement_rejects_edge_and_size_without_global_true() {
    for action in [
        r#"NewPane { edge "left"; size "25%"; }"#,
        r#"Run "bash" { edge "left"; size "25%"; }"#,
    ] {
        let source = format!(
            r#"
                keybinds {{
                    normal {{
                        bind "Ctrl g" {{ {action}; }}
                    }}
                }}
            "#
        );
        let document: KdlDocument = source.parse().unwrap();
        assert!(
            Keybinds::from_kdl(
                document.get("keybinds").unwrap(),
                Default::default(),
                &Default::default(),
            )
            .is_err(),
            "expected incomplete global placement to be rejected for {action}"
        );
    }
}

/// `awaits_placement` reaches the configuration map beside `overlay`, and nothing else does.
///
/// The map is plugin identity, and this key has to be in it for the same reason `overlay` is:
/// hocus's two halves are opened both by these keybindings and by each other
/// (`hocus_proto::launch::overlay_configuration`), and a launch whose configuration differs by one
/// key addresses a different instance - which shows up as a duplicate pane rather than an error.
/// The pair asserted below is exactly what that function writes; `floating` and
/// `move_to_focused_tab` are launch properties rather than configuration and stay out of it.
#[test]
fn awaits_placement_reaches_the_configuration_beside_overlay() {
    let binding = |block: &str| {
        let source = format!(
            r#"
            keybinds {{
                normal {{
                    bind "Ctrl o" {{
                        LaunchOrFocusPlugin "hocus" {{
                            {block}
                        }}
                    }}
                }}
            }}
        "#
        );
        let document: KdlDocument = source.parse().unwrap();
        let keybinds = Keybinds::from_kdl(
            document.get("keybinds").unwrap(),
            Default::default(),
            &Default::default(),
        )
        .unwrap();
        let key = KeyWithModifier::new(BareKey::Char('o')).with_ctrl_modifier();
        let Some(Action::LaunchOrFocusPlugin { plugin, .. }) = keybinds
            .get_actions_for_key_in_mode(&InputMode::Normal, &key)
            .and_then(|actions| actions.first())
        else {
            panic!("expected LaunchOrFocusPlugin");
        };
        match plugin {
            RunPluginOrAlias::RunPlugin(plugin) => plugin.configuration.inner().clone(),
            RunPluginOrAlias::Alias(alias) => alias
                .configuration
                .as_ref()
                .map(|configuration| configuration.inner().clone())
                .unwrap_or_default(),
        }
    };

    let overlay = binding(
        "floating true
                            overlay true
                            awaits_placement true
                            move_to_focused_tab true",
    );
    assert_eq!(
        overlay,
        [
            ("awaits_placement".to_owned(), "true".to_owned()),
            ("overlay".to_owned(), "true".to_owned()),
        ]
        .into_iter()
        .collect::<std::collections::BTreeMap<_, _>>(),
        "character for character what `overlay_configuration` writes, and nothing besides"
    );

    // and it survives a dump-and-reload, or `zellij setup --dump-config` would hand the user a
    // config whose `c` binding addressed a different instance from the one they were running
    let source = r#"
        keybinds {
            normal {
                bind "Ctrl o" {
                    LaunchOrFocusPlugin "hocus" {
                        floating true
                        overlay true
                        awaits_placement true
                        move_to_focused_tab true
                    }
                }
            }
        }
    "#;
    let document: KdlDocument = source.parse().unwrap();
    let keybinds = Keybinds::from_kdl(
        document.get("keybinds").unwrap(),
        Default::default(),
        &Default::default(),
    )
    .unwrap();
    let serialized = Keybinds::to_kdl(&keybinds, true).to_string();
    assert!(
        serialized.contains("awaits_placement true"),
        "not in the dump: {serialized}"
    );
}
