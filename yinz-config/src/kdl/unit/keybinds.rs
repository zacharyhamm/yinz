use super::*;

#[test]
fn legacy_toggle_mother_binding_round_trips_as_noop() {
    let config = Config::from_kdl(
        r#"
        keybinds clear-defaults=true {
            agent {
                bind "m" { ToggleMother; SwitchToMode "Normal"; }
            }
        }
        "#,
        None,
    )
    .unwrap();
    assert_eq!(
        config.keybinds.0[&InputMode::Agent][&KeyWithModifier::new(BareKey::Char('m'))],
        vec![
            Action::NoOp,
            Action::SwitchToMode {
                input_mode: InputMode::Normal,
            },
        ]
    );

    let serialized = config.to_string(false).unwrap();
    assert!(serialized.contains("NoOp"));
    assert!(!serialized.contains("ToggleMother"));
    let reparsed = Config::from_kdl(&serialized, None).unwrap();
    assert_eq!(config.keybinds, reparsed.keybinds);
}

#[test]
fn legacy_toggle_mother_rejects_positional_arguments() {
    for argument in [r#""unexpected""#, "1", "true"] {
        assert!(Config::from_kdl(
            &format!("keybinds {{ agent {{ bind \"m\" {{ ToggleMother {argument}; }}; }}; }}"),
            None,
        )
        .is_err());
    }
}

#[test]
fn plugin_messages_preserve_optional_in_place_placement() {
    for message in ["MessagePlugin \"test-plugin\"", "MessagePluginId 3"] {
        for in_place in [None, Some(false), Some(true)] {
            let flag = in_place
                .map(|value| format!("in_place {value};"))
                .unwrap_or_default();
            let config = Config::from_kdl(
                &format!(
                    "keybinds clear-defaults=true {{ normal {{ bind \"a\" {{ {message} {{ name \"test\"; {flag} }}; }}; }}; }}"
                ),
                None,
            )
            .unwrap();
            let action = &config.keybinds.0[&InputMode::Normal]
                [&KeyWithModifier::new(BareKey::Char('a'))][0];
            assert!(
                matches!(action, Action::KeybindPipe { in_place: actual, .. } if *actual == in_place)
            );
            let serialized = crate::kdl::keybinds::to_kdl(&config.keybinds, true).unwrap();
            let reread = crate::kdl::keybinds::from_kdl(
                &serialized,
                Default::default(),
                &Default::default(),
            )
            .unwrap();
            assert_eq!(config.keybinds, reread);
        }
    }
}

const NEW_TAB_LAYOUT: &str = r#"
layout {
    pane command="printf" { args "tiled"; }
    pane command="cat"
    floating_panes {
        pane command="printf" { args "floating"; }
    }
    swap_tiled_layout name="columns" {
        tab max_panes=3 { pane; pane; pane; }
    }
    swap_floating_layout name="floats" {
        floating_panes max_panes=2 { pane; pane; }
    }
}
"#;

fn new_tab_config(metadata: &str, layout_dir: &std::path::Path) -> Config {
    Config::from_kdl(
        &format!(
            r#"
            layout_dir {:?}
            default_layout "custom"
            keybinds clear-defaults=true {{
                normal {{
                    bind "Ctrl t" {{ NewTab {metadata}; SwitchToMode "Normal"; }}
                }}
            }}
            "#,
            layout_dir.display().to_string(),
        ),
        None,
    )
    .unwrap()
}

#[test]
fn new_tab_layout_references_survive_rebinding_and_saving() {
    let dir = tempfile::tempdir().unwrap();
    let layout_path = dir.path().join("custom.kdl");
    std::fs::write(&layout_path, NEW_TAB_LAYOUT).unwrap();
    let config_path = dir.path().join("config.kdl");

    for reference in ["custom".to_owned(), layout_path.display().to_string()] {
        let mut config = new_tab_config(
            &format!(
                r#"{{ layout {reference:?}; name "work"; cwd "/tmp"; should_change_focus_to_new_tab false; }}"#
            ),
            dir.path(),
        );
        let binding = &config.keybinds.0[&InputMode::Normal]
            [&KeyWithModifier::new(BareKey::Char('t')).with_ctrl_modifier()];
        match &binding[0] {
            Action::NewTab {
                layout_source,
                tiled_layout,
                floating_layouts,
                swap_tiled_layouts,
                swap_floating_layouts,
                tab_name,
                should_change_focus_to_new_tab,
                cwd,
                ..
            } => {
                assert_eq!(
                    layout_source.as_ref().unwrap(),
                    &std::path::PathBuf::from(&reference)
                );
                assert_eq!(tiled_layout.as_ref().unwrap().children.len(), 2);
                assert_eq!(floating_layouts.len(), 1);
                assert_eq!(swap_tiled_layouts.as_ref().unwrap().len(), 1);
                assert_eq!(swap_floating_layouts.as_ref().unwrap().len(), 1);
                assert_eq!(tab_name.as_deref(), Some("work"));
                assert!(!should_change_focus_to_new_tab);
                assert_eq!(cwd.as_deref(), Some(std::path::Path::new("/tmp")));
            },
            action => panic!("expected NewTab, got {action:?}"),
        }
        // Saving an unrelated key rebind must preserve the original tab action.
        config
            .keybinds
            .0
            .get_mut(&InputMode::Normal)
            .unwrap()
            .insert(
                KeyWithModifier::new(BareKey::Char('x')).with_ctrl_modifier(),
                vec![Action::Copy],
            );
        let serialized = config.to_string(false).unwrap();
        assert_eq!(
            config.keybinds,
            Config::from_kdl(&serialized, None).unwrap().keybinds
        );
        std::fs::write(&config_path, "// existing config\n").unwrap();
        let written = config
            .write_config_to_disk(serialized, &config_path)
            .unwrap();
        assert_eq!(config.keybinds, written.keybinds);
        assert_eq!(
            config.keybinds,
            Config::from_kdl(&std::fs::read_to_string(&config_path).unwrap(), None)
                .unwrap()
                .keybinds
        );
    }
}

#[test]
fn new_tab_bare_implicit_default_and_focus_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("custom.kdl"), NEW_TAB_LAYOUT).unwrap();
    for metadata in [
        "",
        "{}",
        r#"{ name "work"; }"#,
        r#"{ should_change_focus_to_new_tab false; }"#,
    ] {
        let config = new_tab_config(metadata, dir.path());
        let reparsed = Config::from_kdl(&config.to_string(false).unwrap(), None).unwrap();
        assert_eq!(config.keybinds, reparsed.keybinds, "{metadata}");
        let binding = &reparsed.keybinds.0[&InputMode::Normal]
            [&KeyWithModifier::new(BareKey::Char('t')).with_ctrl_modifier()];
        assert!(matches!(
            &binding[0],
            Action::NewTab {
                layout_source: None,
                ..
            }
        ));
    }
}

/// A binding built over the plugin wire with a name and no layout means "the default layout,
/// chosen when it runs", which is what its saved text `NewTab { name "work" }` means too, even
/// though parsing that text loads the layout eagerly. One that carries a layout the saved text
/// cannot express is still refused.
#[test]
fn new_tab_wire_binding_without_a_layout_can_be_saved() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("custom.kdl"), NEW_TAB_LAYOUT).unwrap();
    let mut config = new_tab_config(r#"{ name "work"; }"#, dir.path());
    let key = KeyWithModifier::new(BareKey::Char('t')).with_ctrl_modifier();
    let binding = config
        .keybinds
        .0
        .get_mut(&InputMode::Normal)
        .unwrap()
        .get_mut(&key)
        .unwrap();
    let Action::NewTab {
        tiled_layout,
        floating_layouts,
        swap_tiled_layouts,
        swap_floating_layouts,
        ..
    } = &mut binding[0]
    else {
        panic!("not a NewTab binding");
    };
    let floating = floating_layouts.clone();
    *tiled_layout = None;
    floating_layouts.clear();
    *swap_tiled_layouts = None;
    *swap_floating_layouts = None;
    config
        .write_config_to_disk(
            config.to_string(false).unwrap(),
            &dir.path().join("config.kdl"),
        )
        .expect("an implicit default layout is what the saved text loads");

    let binding = config
        .keybinds
        .0
        .get_mut(&InputMode::Normal)
        .unwrap()
        .get_mut(&key)
        .unwrap();
    let Action::NewTab {
        floating_layouts, ..
    } = &mut binding[0]
    else {
        panic!("not a NewTab binding");
    };
    *floating_layouts = floating;
    assert!(
        config
            .write_config_to_disk(
                config.to_string(false).unwrap(),
                &dir.path().join("config.kdl")
            )
            .is_err(),
        "the saved text drops the floating layout"
    );
}

#[test]
fn new_tab_explicit_focus_overrides_layout_focus() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("custom.kdl"),
        "layout { tab focus=false { pane; }; }\n",
    )
    .unwrap();
    for focus in [false, true] {
        let config = new_tab_config(
            &format!("{{ should_change_focus_to_new_tab {focus}; }}"),
            dir.path(),
        );
        let reparsed = Config::from_kdl(&config.to_string(false).unwrap(), None).unwrap();
        assert_eq!(config.keybinds, reparsed.keybinds);
        let binding = &reparsed.keybinds.0[&InputMode::Normal]
            [&KeyWithModifier::new(BareKey::Char('t')).with_ctrl_modifier()];
        assert!(
            matches!(&binding[0], Action::NewTab { should_change_focus_to_new_tab, .. } if *should_change_focus_to_new_tab == focus)
        );
    }
}

#[test]
fn new_tab_relative_layout_source_is_kept_as_written() {
    let dir = tempfile::tempdir_in(".").unwrap();
    std::fs::write(dir.path().join("custom.kdl"), NEW_TAB_LAYOUT).unwrap();
    let layout_path = std::path::Path::new(".")
        .join(dir.path().file_name().unwrap())
        .join("custom.kdl");
    assert!(layout_path.is_relative() && layout_path.exists());
    let reference = layout_path.display().to_string();
    let config = new_tab_config(&format!("{{ layout {reference:?}; }}"), dir.path());
    let binding = &config.keybinds.0[&InputMode::Normal]
        [&KeyWithModifier::new(BareKey::Char('t')).with_ctrl_modifier()];
    match &binding[0] {
        Action::NewTab {
            layout_source,
            tiled_layout,
            ..
        } => {
            assert_eq!(layout_source.as_ref().unwrap(), &layout_path);
            assert_eq!(tiled_layout.as_ref().unwrap().children.len(), 2);
        },
        _ => panic!("expected NewTab"),
    }
    let serialized = config.to_string(false).unwrap();
    assert!(
        serialized.contains(&format!("layout {reference:?}")),
        "{serialized}"
    );
    assert_eq!(
        config.keybinds,
        Config::from_kdl(&serialized, None).unwrap().keybinds
    );
}

#[test]
fn new_tab_lossy_save_leaves_existing_config_and_backups_untouched() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("custom.kdl"), NEW_TAB_LAYOUT).unwrap();
    let config_path = dir.path().join("config.kdl");
    let existing = "// do not overwrite\n";
    std::fs::write(&config_path, existing).unwrap();
    // A bare NewTab in the saved text would drop the binding's referenced layout.
    let config = new_tab_config(r#"{ layout "custom"; }"#, dir.path());
    let lossy = new_tab_config("", dir.path()).to_string(false).unwrap();
    assert!(config.write_config_to_disk(lossy, &config_path).is_err());
    // An implicit default layout is saved as `NewTab {}`, which loads whatever the default
    // layout is at load time: a binding whose layout is not that cannot be saved faithfully.
    let config = new_tab_config("{}", dir.path());
    std::fs::write(dir.path().join("custom.kdl"), "layout { pane; }\n").unwrap();
    assert!(config
        .write_config_to_disk(config.to_string(false).unwrap(), &config_path)
        .is_err());
    assert_eq!(std::fs::read_to_string(&config_path).unwrap(), existing);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
}

#[test]
fn new_tab_inline_layout_without_a_reference_cannot_be_saved() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("custom.kdl"), NEW_TAB_LAYOUT).unwrap();
    let config_path = dir.path().join("config.kdl");
    let mut config = new_tab_config("{}", dir.path());
    let binding = config
        .keybinds
        .0
        .get_mut(&InputMode::Normal)
        .unwrap()
        .get_mut(&KeyWithModifier::new(BareKey::Char('t')).with_ctrl_modifier())
        .unwrap();
    // A plugin can bind a layout it built itself, which no saved text can reproduce.
    if let Action::NewTab { tiled_layout, .. } = &mut binding[0] {
        tiled_layout.as_mut().unwrap().children.clear();
    }
    assert!(config
        .write_config_to_disk(config.to_string(false).unwrap(), &config_path)
        .is_err());
    assert!(!config_path.exists());
}

#[test]
fn new_tab_referenced_layout_can_be_saved_after_the_file_changed() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("custom.kdl"), NEW_TAB_LAYOUT).unwrap();
    let config_path = dir.path().join("config.kdl");
    let config = new_tab_config(r#"{ layout "custom"; name "work"; }"#, dir.path());
    std::fs::write(dir.path().join("custom.kdl"), "layout { pane; }\n").unwrap();
    let written = config
        .write_config_to_disk(config.to_string(false).unwrap(), &config_path)
        .unwrap();
    let saved = std::fs::read_to_string(&config_path).unwrap();
    assert!(saved.contains(r#"layout "custom""#), "{saved}");
    let binding = &written.keybinds.0[&InputMode::Normal]
        [&KeyWithModifier::new(BareKey::Char('t')).with_ctrl_modifier()];
    match &binding[0] {
        Action::NewTab {
            layout_source,
            tiled_layout,
            tab_name,
            ..
        } => {
            assert_eq!(
                layout_source.as_deref(),
                Some(std::path::Path::new("custom"))
            );
            assert_eq!(tab_name.as_deref(), Some("work"));
            // The saved reference loads the file as it is now.
            assert_eq!(tiled_layout.as_ref().unwrap().children.len(), 1);
        },
        action => panic!("expected NewTab, got {action:?}"),
    }
}

#[test]
fn keybinds_to_string() {
    let fake_config = r#"
        keybinds {
            normal {
                bind "Ctrl g" { SwitchToMode "Locked"; }
            }
        }"#;
    let document: KdlDocument = fake_config.parse().unwrap();
    let deserialized = crate::kdl::keybinds::from_kdl(
        document.get("keybinds").unwrap(),
        Default::default(),
        &Default::default(),
    )
    .unwrap();
    let clear_defaults = true;
    let serialized = crate::kdl::keybinds::to_kdl(&deserialized, clear_defaults).unwrap();
    let deserialized_from_serialized = crate::kdl::keybinds::from_kdl(
        serialized
            .to_string()
            .parse::<KdlDocument>()
            .unwrap()
            .get("keybinds")
            .unwrap(),
        Default::default(),
        &Default::default(),
    )
    .unwrap();
    insta::assert_snapshot!(serialized.to_string());
    assert_eq!(
        deserialized, deserialized_from_serialized,
        "Deserialized serialized config equals original config"
    );
}

#[test]
fn keybinds_to_string_without_clearing_defaults() {
    let fake_config = r#"
        keybinds {
            normal {
                bind "Ctrl g" { SwitchToMode "Locked"; }
            }
        }"#;
    let document: KdlDocument = fake_config.parse().unwrap();
    let deserialized = crate::kdl::keybinds::from_kdl(
        document.get("keybinds").unwrap(),
        Default::default(),
        &Default::default(),
    )
    .unwrap();
    let clear_defaults = false;
    let serialized = crate::kdl::keybinds::to_kdl(&deserialized, clear_defaults).unwrap();
    let deserialized_from_serialized = crate::kdl::keybinds::from_kdl(
        serialized
            .to_string()
            .parse::<KdlDocument>()
            .unwrap()
            .get("keybinds")
            .unwrap(),
        Default::default(),
        &Default::default(),
    )
    .unwrap();
    insta::assert_snapshot!(serialized.to_string());
    assert_eq!(
        deserialized, deserialized_from_serialized,
        "Deserialized serialized config equals original config"
    );
}

#[test]
fn keybinds_to_string_with_multiple_actions() {
    let fake_config = r#"
        keybinds {
            normal {
                bind "Ctrl n" { NewPane; SwitchToMode "Locked"; }
            }
        }"#;
    let document: KdlDocument = fake_config.parse().unwrap();
    let deserialized = crate::kdl::keybinds::from_kdl(
        document.get("keybinds").unwrap(),
        Default::default(),
        &Default::default(),
    )
    .unwrap();
    let clear_defaults = true;
    let serialized = crate::kdl::keybinds::to_kdl(&deserialized, clear_defaults).unwrap();
    let deserialized_from_serialized = crate::kdl::keybinds::from_kdl(
        serialized
            .to_string()
            .parse::<KdlDocument>()
            .unwrap()
            .get("keybinds")
            .unwrap(),
        Default::default(),
        &Default::default(),
    )
    .unwrap();
    assert_eq!(
        deserialized, deserialized_from_serialized,
        "Deserialized serialized config equals original config"
    );
    insta::assert_snapshot!(serialized.to_string());
}

#[test]
fn can_bind_theme_actions() {
    // Regression test for https://github.com/zellij-org/zellij/issues/5297
    // SetDarkTheme / SetLightTheme / ToggleTheme work via the CLI but used to be
    // rejected by the keybinding parser with "Unsupported action".
    let fake_config = r#"
        keybinds {
            normal {
                bind "Ctrl t" { ToggleTheme; }
                bind "Ctrl d" { SetDarkTheme; }
                bind "Ctrl l" { SetLightTheme; }
            }
        }"#;
    let document: KdlDocument = fake_config.parse().unwrap();
    let deserialized = crate::kdl::keybinds::from_kdl(
        document.get("keybinds").unwrap(),
        Default::default(),
        &Default::default(),
    )
    .unwrap();
    let ctrl_t = KeyWithModifier::new(BareKey::Char('t')).with_ctrl_modifier();
    assert_eq!(
        deserialized.get_actions_for_key_in_mode(&InputMode::Normal, &ctrl_t),
        Some(&vec![Action::ToggleTheme])
    );
    let ctrl_d = KeyWithModifier::new(BareKey::Char('d')).with_ctrl_modifier();
    assert_eq!(
        deserialized.get_actions_for_key_in_mode(&InputMode::Normal, &ctrl_d),
        Some(&vec![Action::SetDarkTheme])
    );
    let ctrl_l = KeyWithModifier::new(BareKey::Char('l')).with_ctrl_modifier();
    assert_eq!(
        deserialized.get_actions_for_key_in_mode(&InputMode::Normal, &ctrl_l),
        Some(&vec![Action::SetLightTheme])
    );
    // The bindings must also survive a serialize -> deserialize round-trip.
    let serialized = crate::kdl::keybinds::to_kdl(&deserialized, true).unwrap();
    let deserialized_from_serialized = crate::kdl::keybinds::from_kdl(
        serialized
            .to_string()
            .parse::<KdlDocument>()
            .unwrap()
            .get("keybinds")
            .unwrap(),
        Default::default(),
        &Default::default(),
    )
    .unwrap();
    assert_eq!(deserialized, deserialized_from_serialized);
}

#[test]
fn keybinds_to_string_with_all_actions() {
    let fake_config = r#"
        keybinds {
            normal {
                bind "Ctrl a" { Quit; }
                bind "Ctrl b" { Write 102 111 111; }
                bind "Ctrl c" { WriteChars "hi there!"; }
                bind "Ctrl d" { SwitchToMode "Locked"; }
                bind "Ctrl e" { Resize "Increase"; }
                bind "Ctrl f" { FocusNextPane; }
                bind "Ctrl g" { FocusPreviousPane; }
                bind "Ctrl h" { SwitchFocus; }
                bind "Ctrl i" { MoveFocus "Right"; }
                bind "Ctrl j" { MoveFocusOrTab "Right"; }
                bind "Ctrl k" { MovePane "Right"; }
                bind "Ctrl l" { MovePaneBackwards; }
                bind "Ctrl m" { Resize "Decrease Down"; }
                bind "Ctrl n" { DumpScreen "/tmp/dumped"; }
                bind "Ctrl o" { DumpLayout "/tmp/dumped-layout"; }
                bind "Ctrl p" { EditScrollback; }
                bind "Ctrl q" { ScrollUp; }
                bind "Ctrl r" { ScrollDown; }
                bind "Ctrl s" { ScrollToBottom; }
                bind "Ctrl t" { ScrollToTop; }
                bind "Ctrl u" { PageScrollUp; }
                bind "Ctrl v" { PageScrollDown; }
                bind "Ctrl w" { HalfPageScrollUp; }
                bind "Ctrl x" { HalfPageScrollDown; }
                bind "Ctrl y" { ToggleFocusFullscreen; }
                bind "Ctrl z" { TogglePaneFrames; }
                bind "Alt a" { ToggleActiveSyncTab; }
                bind "Alt b" { NewPane "Right"; }
                bind "Alt c" { TogglePaneEmbedOrFloating; }
                bind "Alt d" { ToggleFloatingPanes; }
                bind "Alt e" { CloseFocus; }
                bind "Alt f" { PaneNameInput 0; }
                bind "Alt g" { UndoRenamePane; }
                bind "Alt h" { NewTab; }
                bind "Alt i" { GoToNextTab; }
                bind "Alt j" { GoToPreviousTab; }
                bind "Alt k" { CloseTab; }
                bind "Alt l" { GoToTab 1; }
                bind "Alt m" { ToggleTab; }
                bind "Alt n" { TabNameInput 0; }
                bind "Alt o" { UndoRenameTab; }
                bind "Alt p" { MoveTab "Right"; }
                bind "Alt q" {
                    Run "ls" "-l" {
                        start_suspended true;
                        close_on_exit true;
                        cwd "/tmp";
                        name "my cool pane";
                    };
                }
                bind "Alt r" {
                    Run "ls" "-l" {
                        start_suspended true;
                        close_on_exit true;
                        cwd "/tmp";
                        name "my cool pane";
                        floating true;
                    };
                }
                bind "Alt s" {
                    Run "ls" "-l" {
                        start_suspended true;
                        close_on_exit true;
                        cwd "/tmp";
                        name "my cool pane";
                        in_place true;
                    };
                }
                bind "Alt t" { Detach; }
                bind "Alt u" {
                    LaunchOrFocusPlugin "yinz:strider"{
                        floating true;
                        move_to_focused_tab true;
                        skip_plugin_cache true;
                        config_key_1 "config_value_1";
                        config_key_2 "config_value_2";
                    };
                }
                bind "Alt v" {
                    LaunchOrFocusPlugin "yinz:strider"{
                        in_place true;
                        move_to_focused_tab true;
                        skip_plugin_cache true;
                        config_key_1 "config_value_1";
                        config_key_2 "config_value_2";
                    };
                }
                bind "Alt w" {
                    LaunchPlugin "yinz:strider" {
                        floating true;
                        skip_plugin_cache true;
                        config_key_1 "config_value_1";
                        config_key_2 "config_value_2";
                    };
                }
                bind "Alt x" {
                    LaunchPlugin "yinz:strider"{
                        in_place true;
                        skip_plugin_cache true;
                        config_key_1 "config_value_1";
                        config_key_2 "config_value_2";
                    };
                }
                bind "Alt y" { Copy; }
                bind "Alt z" { SearchInput 0; }
                bind "Ctrl Alt a" { Search "Up"; }
                bind "Ctrl Alt b" { SearchToggleOption "CaseSensitivity"; }
                bind "Ctrl Alt c" { ToggleMouseMode; }
                bind "Ctrl Alt d" { PreviousSwapLayout; }
                bind "Ctrl Alt e" { NextSwapLayout; }
                bind "Ctrl Alt g" { BreakPane; }
                bind "Ctrl Alt h" { BreakPaneRight; }
                bind "Ctrl Alt i" { BreakPaneLeft; }
                bind "Ctrl Alt i" { BreakPaneLeft; }
                bind "Ctrl Alt j" {
                    MessagePlugin "yinz:strider"{
                        name "message_name";
                        payload "message_payload";
                        cwd "/tmp";
                        launch_new true;
                        skip_cache true;
                        floating true;
                        in_place true;
                        title "plugin_title";
                        config_key_1 "config_value_1";
                        config_key_2 "config_value_2";
                    };
                }
                bind "Ctrl Alt k" { FocusLastPane; }
                bind "Ctrl Alt l" { Clear; }
                bind "Ctrl Alt m" { Confirm; }
                bind "Ctrl Alt n" { Deny; }
                bind "Ctrl Alt o" { NoOp; }
                bind "Ctrl Alt p" { RenameSession "work"; }
                bind "Ctrl Alt q" { MessagePluginId 3 { name "n"; payload "p"; }; }
                bind "Ctrl Alt r" { OverrideLayout; }
            }
        }"#;
    let document: KdlDocument = fake_config.parse().unwrap();
    let deserialized = crate::kdl::keybinds::from_kdl(
        document.get("keybinds").unwrap(),
        Default::default(),
        &Default::default(),
    )
    .unwrap();
    let clear_defaults = true;
    let serialized = crate::kdl::keybinds::to_kdl(&deserialized, clear_defaults).unwrap();
    let deserialized_from_serialized = crate::kdl::keybinds::from_kdl(
        serialized
            .to_string()
            .parse::<KdlDocument>()
            .unwrap()
            .get("keybinds")
            .unwrap(),
        Default::default(),
        &Default::default(),
    )
    .unwrap();
    // uncomment the below lines for more easily debugging a failed assertion here
    //     for (input_mode, input_mode_keybinds) in deserialized.0 {
    //         if let Some(other_input_mode_keybinds) = deserialized_from_serialized.0.get(&input_mode) {
    //             for (keybind, action) in input_mode_keybinds {
    //                 if let Some(other_action) = other_input_mode_keybinds.get(&keybind) {
    //                     assert_eq!(&action, other_action);
    //                 } else {
    //                     eprintln!("keybind: {:?} not found in other", keybind);
    //                 }
    //             }
    //         }
    //     }
    assert_eq!(
        deserialized, deserialized_from_serialized,
        "Deserialized serialized config equals original config"
    );
    insta::assert_snapshot!(serialized.to_string());
}

#[test]
fn keybinds_to_string_with_shared_modes() {
    let fake_config = r#"
        keybinds {
            normal {
                bind "Ctrl n" { NewPane; SwitchToMode "Locked"; }
            }
            locked {
                bind "Ctrl n" { NewPane; SwitchToMode "Locked"; }
            }
            shared_except "locked" "pane" {
                bind "Ctrl f" { TogglePaneEmbedOrFloating; }
            }
            shared_among "locked" "pane" {
                bind "Ctrl p" { WriteChars "foo"; }
            }
        }"#;
    let document: KdlDocument = fake_config.parse().unwrap();
    let deserialized = crate::kdl::keybinds::from_kdl(
        document.get("keybinds").unwrap(),
        Default::default(),
        &Default::default(),
    )
    .unwrap();
    let clear_defaults = true;
    let serialized = crate::kdl::keybinds::to_kdl(&deserialized, clear_defaults).unwrap();
    let deserialized_from_serialized = crate::kdl::keybinds::from_kdl(
        serialized
            .to_string()
            .parse::<KdlDocument>()
            .unwrap()
            .get("keybinds")
            .unwrap(),
        Default::default(),
        &Default::default(),
    )
    .unwrap();
    assert_eq!(
        deserialized, deserialized_from_serialized,
        "Deserialized serialized config equals original config"
    );
    insta::assert_snapshot!(serialized.to_string());
}

#[test]
fn message_plugin_in_place_is_read_and_written_back() {
    // `in_place` used to parse to `None` here while the same child key worked on
    // `Run`/`LaunchPlugin`, so the plugin opened floating with no diagnostic.
    let fake_config = r#"
        keybinds {
            normal {
                bind "Ctrl y" {
                    MessagePlugin "yinz:strider" {
                        name "go";
                        in_place true;
                    };
                }
            }
        }"#;
    let document: KdlDocument = fake_config.parse().unwrap();
    let deserialized = crate::kdl::keybinds::from_kdl(
        document.get("keybinds").unwrap(),
        Default::default(),
        &Default::default(),
    )
    .unwrap();
    let key = KeyWithModifier::new(BareKey::Char('y')).with_ctrl_modifier();
    let actions = deserialized
        .get_actions_for_key_in_mode(&InputMode::Normal, &key)
        .expect("the binding");
    match &actions[0] {
        Action::KeybindPipe { in_place, .. } => assert_eq!(*in_place, Some(true)),
        other => panic!("expected a pipe, got {other:?}"),
    }
    let serialized = crate::kdl::keybinds::to_kdl(&deserialized, true).unwrap();
    assert!(
        serialized.to_string().contains("in_place true"),
        "{serialized}"
    );
}

#[test]
fn keybinds_to_string_with_multiple_multiline_actions() {
    let fake_config = r#"
        keybinds {
            shared {
                bind "Ctrl n" {
                    NewPane
                    SwitchToMode "Locked"
                    MessagePlugin "yinz:strider"{
                        name "message_name";
                        payload "message_payload";
                        cwd "/tmp";
                        launch_new true;
                        skip_cache true;
                        floating true;
                        in_place true;
                        title "plugin_title";
                        config_key_1 "config_value_1";
                        config_key_2 "config_value_2";
                    };
                }
            }
        }"#;
    let document: KdlDocument = fake_config.parse().unwrap();
    let deserialized = crate::kdl::keybinds::from_kdl(
        document.get("keybinds").unwrap(),
        Default::default(),
        &Default::default(),
    )
    .unwrap();
    let clear_defaults = true;
    let serialized = crate::kdl::keybinds::to_kdl(&deserialized, clear_defaults).unwrap();
    let deserialized_from_serialized = crate::kdl::keybinds::from_kdl(
        serialized
            .to_string()
            .parse::<KdlDocument>()
            .unwrap()
            .get("keybinds")
            .unwrap(),
        Default::default(),
        &Default::default(),
    )
    .unwrap();
    assert_eq!(
        deserialized, deserialized_from_serialized,
        "Deserialized serialized config equals original config"
    );
    insta::assert_snapshot!(serialized.to_string());
}

fn keybinds_from(config: &str) -> Keybinds {
    let document: KdlDocument = config.parse().unwrap();
    crate::kdl::keybinds::from_kdl(
        document.get("keybinds").unwrap(),
        Default::default(),
        &Default::default(),
    )
    .unwrap()
}

fn reparsed(keybinds: &Keybinds) -> Keybinds {
    let serialized = crate::kdl::keybinds::to_kdl(keybinds, true).unwrap();
    keybinds_from(&serialized.to_string())
}

fn config_with_binding(actions: Vec<Action>) -> Config {
    let mut config = Config::from_kdl(
        r#"keybinds clear-defaults=true { normal { bind "Ctrl x" { Copy; }; }; }"#,
        None,
    )
    .unwrap();
    config
        .keybinds
        .0
        .get_mut(&InputMode::Normal)
        .unwrap()
        .insert(
            KeyWithModifier::new(BareKey::Char('x')).with_ctrl_modifier(),
            actions,
        );
    config
}

/// Every name the parser accepts has a writer, so a binding that loads is a binding that saves.
/// These are the ones that used to fall through the serializer's wildcard arm and vanish.
#[test]
fn named_actions_without_a_serializer_round_trip() {
    let keybinds = keybinds_from(
        r#"
        keybinds clear-defaults=true {
            normal {
                bind "Ctrl a" { Clear; }
                bind "Ctrl b" { Confirm; }
                bind "Ctrl c" { Deny; }
                bind "Ctrl d" { NoOp; }
                bind "Ctrl e" { RenameSession "work"; }
                bind "Ctrl f" { MessagePluginId 3 { name "n"; payload "p"; }; }
                bind "Ctrl g" { OverrideLayout; }
            }
        }"#,
    );
    let serialized = crate::kdl::keybinds::to_kdl(&keybinds, true)
        .unwrap()
        .to_string();
    assert!(serialized.contains("{ Clear; }"), "{serialized}");
    assert!(
        serialized.contains(r#"RenameSession "work""#),
        "{serialized}"
    );
    assert!(serialized.contains("MessagePluginId 3"), "{serialized}");
    assert_eq!(keybinds_from(&serialized), keybinds, "{serialized}");
}

/// The writer used to spell these `hold_on_start`/`hold_on_close`, two names the parser has
/// never read, so both flags reverted to their defaults on every save.
#[test]
fn run_action_start_suspended_and_close_on_exit_round_trip() {
    let keybinds = keybinds_from(
        r#"
        keybinds clear-defaults=true {
            normal {
                bind "Ctrl r" { Run "ls" { start_suspended true; close_on_exit true; }; }
            }
        }"#,
    );
    let command =
        keybinds.0[&InputMode::Normal]
            .values()
            .flatten()
            .find_map(|action| match action {
                Action::NewTiledPane { command, .. } => command.clone(),
                _ => None,
            });
    let command = command.expect("the Run action was not parsed");
    assert!(command.hold_on_start && !command.hold_on_close);
    let serialized = crate::kdl::keybinds::to_kdl(&keybinds, true)
        .unwrap()
        .to_string();
    assert!(serialized.contains("start_suspended true"), "{serialized}");
    assert!(serialized.contains("close_on_exit true"), "{serialized}");
    assert!(!serialized.contains("hold_on"), "{serialized}");
    assert_eq!(keybinds_from(&serialized), keybinds, "{serialized}");
}

/// A floating `Run`'s coordinates are written the way the parser reads them - as strings - and
/// `pinned`/`borderless`, which were parsed but never written, survive too.
#[test]
fn floating_run_coordinates_pinned_and_borderless_round_trip() {
    let keybinds = keybinds_from(
        r#"
        keybinds clear-defaults=true {
            normal {
                bind "Ctrl r" {
                    Run "ls" {
                        floating true
                        x "10%"
                        y "3"
                        width "50%"
                        height "20"
                        pinned true
                        borderless false
                    }
                }
            }
        }"#,
    );
    let placement =
        keybinds.0[&InputMode::Normal]
            .values()
            .flatten()
            .find_map(|action| match action {
                Action::NewFloatingPane { placement, .. } => Some(placement.clone()),
                _ => None,
            });
    let coordinates = placement
        .expect("the Run action was not parsed")
        .floating_pane_coordinates()
        .expect("the coordinates were not parsed");
    assert_eq!(coordinates.x, Some(PercentOrFixed::Percent(10)));
    assert_eq!(coordinates.height, Some(PercentOrFixed::Fixed(20)));
    assert_eq!(coordinates.pinned, Some(true));
    assert_eq!(coordinates.borderless, Some(false));
    assert_eq!(reparsed(&keybinds), keybinds);
}

/// `GoToTab` used to go through the byte-list parser the input actions share, which folded
/// `GoToTab 300` into tab 44 without a word.
#[test]
fn go_to_tab_reads_the_whole_number() {
    let keybinds = keybinds_from(
        r#"keybinds clear-defaults=true { normal { bind "Ctrl t" { GoToTab 300; }; }; }"#,
    );
    assert_eq!(
        keybinds.0[&InputMode::Normal]
            [&KeyWithModifier::new(BareKey::Char('t')).with_ctrl_modifier()],
        vec![Action::GoToTab {
            index: yinz_types::internal_ids::TabNumber::new(300)
        }]
    );
    assert_eq!(reparsed(&keybinds), keybinds);
    for config in [
        r#"keybinds { normal { bind "Ctrl t" { GoToTab; }; }; }"#,
        r#"keybinds { normal { bind "Ctrl t" { GoToTab -1; }; }; }"#,
        r#"keybinds { normal { bind "Ctrl t" { GoToTab 4294967296; }; }; }"#,
    ] {
        let document: KdlDocument = config.parse().unwrap();
        assert!(
            crate::kdl::keybinds::from_kdl(
                document.get("keybinds").unwrap(),
                Default::default(),
                &Default::default(),
            )
            .is_err(),
            "{config}"
        );
    }
}

/// A binding holding an action with no config spelling refuses to save, naming the action
/// and where it sits - it is not left out of the file.
#[test]
fn saving_a_wire_only_binding_is_refused_and_names_it() {
    let config = config_with_binding(vec![Action::Copy, Action::ListClients]);
    let error = config.to_string(false).unwrap_err().to_string();
    for expected in ["ListClients", "Ctrl x", "normal", "action 1"] {
        assert!(error.contains(expected), "{error}");
    }

    // A variant that does have a spelling, in a shape the spelling cannot carry.
    let config = config_with_binding(vec![Action::NewTiledPane {
        direction: None,
        command: None,
        pane_name: None,
        near_current_pane: false,
        no_focus: false,
        borderless: None,
        tab_id: None,
    }]);
    let error = config.to_string(false).unwrap_err().to_string();
    assert!(error.contains("NewTiledPane"), "{error}");
    assert!(error.contains("needs a command"), "{error}");
}

/// The write check compares the text against the source, not against itself: text that parses
/// cleanly but says something else is refused before the file is touched.
#[test]
fn saving_a_binding_the_written_form_changes_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.kdl");
    let existing = "// do not overwrite\n";
    std::fs::write(&config_path, existing).unwrap();
    let config = config_with_binding(vec![Action::Copy]);
    let faithful = config.to_string(false).unwrap();
    let edited = faithful.replace(r#"bind "Ctrl x" { Copy; }"#, r#"bind "Ctrl x" { Quit; }"#);
    assert_ne!(edited, faithful, "{faithful}");
    let error = config
        .write_config_to_disk(edited, &config_path)
        .unwrap_err();
    assert!(error.reason.contains("Ctrl x"), "{}", error.reason);
    assert_eq!(error.path.as_deref(), Some(config_path.as_path()));
    assert_eq!(std::fs::read_to_string(&config_path).unwrap(), existing);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    config.write_config_to_disk(faithful, &config_path).unwrap();
}

#[test]
fn context_menu_item_with_a_wire_only_action_is_refused() {
    use yinz_types::data::ContextMenuItem;
    use yinz_types::input::context_menu::ContextMenu;
    let config = Config {
        context_menu: ContextMenu::from_items(vec![ContextMenuItem::new(
            "List them",
            vec![Action::ListClients],
        )]),
        ..Default::default()
    };
    let error = config.to_string(false).unwrap_err().to_string();
    assert!(error.contains("List them"), "{error}");
    assert!(error.contains("ListClients"), "{error}");
}

#[test]
fn run_rejects_retired_firecracker_children_regardless_of_value_or_placement() {
    for placement in ["", "floating true;", "in_place true;", "stacked true;"] {
        for field in [
            "firecracker_vm",
            "firecracker_image",
            "firecracker_worktree",
        ] {
            for value in ["\"vm\"", "\"\"", "", "null", "42", "true"] {
                let text = format!("Run \"ls\" {{ {placement} {field} {value}; }}");
                let doc: KdlDocument = text.parse().unwrap();
                let node = doc.get("Run").unwrap();
                let child = node.children().unwrap().get(field).unwrap();
                let error = crate::kdl::action::from_kdl((node, &Options::default())).unwrap_err();
                let ConfigError::KdlError(error) = error else {
                    panic!("expected a configuration error")
                };
                assert!(error.error_message.contains(field));
                assert!(error
                    .error_message
                    .contains("Firecracker execution is no longer supported."));
                assert_eq!(error.offset, Some(child.span().offset()));
                assert_eq!(error.len, Some(child.span().len()));
            }
        }
        let text = format!("Run \"ls\" {{ {placement} cwd \"/tmp\"; }}");
        let doc: KdlDocument = text.parse().unwrap();
        assert!(
            crate::kdl::action::from_kdl((doc.get("Run").unwrap(), &Options::default())).is_ok()
        );
    }
}

#[test]
fn tab_group_actions_round_trip_kdl_and_default_bindings() {
    use yinz_types::internal_ids::TabNumber;
    let actions = [
        Action::NewTabGroup,
        Action::GoToNextTabGroup,
        Action::MovePaneToNextTabGroup,
        Action::MovePaneToPreviousTabGroup,
        Action::GoToPreviousTabGroup,
        Action::GoToTabInGroup {
            index: TabNumber::new(9),
        },
        Action::SwitchTabGroup { id: u32::MAX },
        Action::RenameTabGroup {
            id: 7,
            name: "工程".into(),
        },
        Action::CloseTabGroup { id: 7 },
    ];
    for action in actions {
        let node = crate::kdl::action::to_kdl(&action).unwrap();
        assert_eq!(
            crate::kdl::action::from_kdl((&node, &Options::default())).unwrap(),
            action
        );
    }
    let config =
        Config::from_kdl(include_str!("../../../assets/config/default.kdl"), None).unwrap();
    let tab = &config.keybinds.0[&InputMode::Tab];
    assert!(tab
        .values()
        .any(|actions| actions.contains(&Action::NewTabGroup)));
    for (key, action) in [
        ('{', Action::MovePaneToPreviousTabGroup),
        ('}', Action::MovePaneToNextTabGroup),
    ] {
        assert_eq!(
            tab.get(&KeyWithModifier::new(BareKey::Char(key))),
            Some(&vec![
                action,
                Action::SwitchToMode {
                    input_mode: InputMode::Normal
                }
            ])
        );
    }
    let saved = Config::from_kdl(&config.to_string(false).unwrap(), None).unwrap();
    assert_eq!(config.keybinds, saved.keybinds);
    for config in [&config, &saved] {
        for (shortcut, action) in [
            ("Alt Shift h", Action::GoToPreviousTabGroup),
            ("Alt Shift l", Action::GoToNextTabGroup),
        ] {
            let key: KeyWithModifier = shortcut.parse().unwrap();
            for (mode, bindings) in &config.keybinds.0 {
                let expected = if matches!(mode, InputMode::Locked | InputMode::Select) {
                    None
                } else {
                    Some(vec![action.clone()])
                };
                assert_eq!(
                    bindings.get(&key),
                    expected.as_ref(),
                    "{mode:?}: {shortcut}"
                );
            }
        }
        for shortcut in ["H", "L"] {
            assert!(!config.keybinds.0[&InputMode::Tab]
                .contains_key(&shortcut.parse::<KeyWithModifier>().unwrap()));
        }
    }
}
