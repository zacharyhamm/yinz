use super::*;

#[test]
fn launch_or_focus_plugin_preserves_overlay_and_modal_configuration() {
    let source = r#"
        keybinds {
            normal {
                bind "Ctrl o" {
                    LaunchOrFocusPlugin "file-finder" {
                        floating true
                        overlay true
                        modal true
                    }
                }
            }
        }
    "#;
    let document: KdlDocument = source.parse().unwrap();
    let keybinds = crate::kdl::keybinds::from_kdl(
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
    let serialized = crate::kdl::keybinds::to_kdl(&keybinds, true)
        .unwrap()
        .to_string();
    assert!(serialized.contains("overlay true"));
    assert!(serialized.contains("modal true"));
    let reparsed = crate::kdl::keybinds::from_kdl(
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
fn plugin_configuration_keys_named_global_edge_and_size_round_trip() {
    let source = r#"
        keybinds {
            normal {
                bind "Ctrl g" {
                    LaunchOrFocusPlugin "yinz:strider" {
                        global false
                        edge "soft"
                        size "large"
                    }
                }
            }
        }
    "#;
    let document: KdlDocument = source.parse().unwrap();
    let keybinds = crate::kdl::keybinds::from_kdl(
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
    let Action::LaunchOrFocusPlugin { plugin, .. } = action else {
        panic!("expected LaunchOrFocusPlugin");
    };
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

    let serialized = crate::kdl::keybinds::to_kdl(&keybinds, true)
        .unwrap()
        .to_string();
    assert!(serialized.contains("global \"false\""));
    assert!(serialized.contains("edge \"soft\""));
    assert!(serialized.contains("size \"large\""));
    let reparsed = crate::kdl::keybinds::from_kdl(
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

/// `awaits_placement` reaches the configuration map beside `overlay`, and nothing else does.
///
/// The map is plugin identity, and this key has to be in it for the same reason `overlay` is:
/// agent's two halves are opened both by these keybindings and by each other
/// (`agent_proto::launch::overlay_configuration`), and a launch whose configuration differs by one
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
                        LaunchOrFocusPlugin "configuration" {{
                            {block}
                        }}
                    }}
                }}
            }}
        "#
        );
        let document: KdlDocument = source.parse().unwrap();
        let keybinds = crate::kdl::keybinds::from_kdl(
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

    // and it survives a dump-and-reload, or `yinz setup --dump-config` would hand the user a
    // config whose `c` binding addressed a different instance from the one they were running
    let source = r#"
        keybinds {
            normal {
                bind "Ctrl o" {
                    LaunchOrFocusPlugin "configuration" {
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
    let keybinds = crate::kdl::keybinds::from_kdl(
        document.get("keybinds").unwrap(),
        Default::default(),
        &Default::default(),
    )
    .unwrap();
    let serialized = crate::kdl::keybinds::to_kdl(&keybinds, true)
        .unwrap()
        .to_string();
    assert!(
        serialized.contains("awaits_placement true"),
        "not in the dump: {serialized}"
    );
}
