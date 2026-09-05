use super::*;

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
        let serialized = config.to_string(false);
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
        let reparsed = Config::from_kdl(&config.to_string(false), None).unwrap();
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
        .write_config_to_disk(config.to_string(false), &dir.path().join("config.kdl"))
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
            .write_config_to_disk(config.to_string(false), &dir.path().join("config.kdl"))
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
        let reparsed = Config::from_kdl(&config.to_string(false), None).unwrap();
        assert_eq!(config.keybinds, reparsed.keybinds);
        let binding = &reparsed.keybinds.0[&InputMode::Normal]
            [&KeyWithModifier::new(BareKey::Char('t')).with_ctrl_modifier()];
        assert!(
            matches!(&binding[0], Action::NewTab { should_change_focus_to_new_tab, .. } if *should_change_focus_to_new_tab == focus)
        );
    }
}

#[test]
fn new_tab_without_swap_layouts_can_be_saved_after_ipc() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("custom.kdl"), "layout { pane; }\n").unwrap();
    let mut config = new_tab_config(r#"{ layout "custom"; }"#, dir.path());
    let binding = config
        .keybinds
        .0
        .get_mut(&InputMode::Normal)
        .unwrap()
        .get_mut(&KeyWithModifier::new(BareKey::Char('t')).with_ctrl_modifier())
        .unwrap();
    // The plugin action wire, like IPC, represents absent swap lists as empty lists.
    let wire: crate::plugin_api::action::ProtobufAction = binding[0].clone().try_into().unwrap();
    binding[0] = wire.try_into().unwrap();
    config
        .write_config_to_disk(config.to_string(false), &dir.path().join("config.kdl"))
        .unwrap();
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
    let serialized = config.to_string(false);
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
    let lossy = new_tab_config("", dir.path()).to_string(false);
    assert!(config.write_config_to_disk(lossy, &config_path).is_err());
    // An implicit default layout is saved as `NewTab {}`, which loads whatever the default
    // layout is at load time: a binding whose layout is not that cannot be saved faithfully.
    let config = new_tab_config("{}", dir.path());
    std::fs::write(dir.path().join("custom.kdl"), "layout { pane; }\n").unwrap();
    assert!(config
        .write_config_to_disk(config.to_string(false), &config_path)
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
        .write_config_to_disk(config.to_string(false), &config_path)
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
        .write_config_to_disk(config.to_string(false), &config_path)
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
fn new_tab_with_pane_colours_can_be_saved_after_ipc() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("custom.kdl"),
        r##"
layout {
    pane default_fg="#ff0000" default_bg="#000000" { pane; pane default_fg="#00ff00"; }
    floating_panes {
        pane default_fg="#0000ff" default_bg="#ffffff"
    }
    swap_tiled_layout name="columns" {
        tab max_panes=3 { pane default_bg="#101010"; pane; pane; }
    }
    swap_floating_layout name="floats" {
        floating_panes max_panes=2 { pane default_fg="#202020"; pane; }
    }
}
"##,
    )
    .unwrap();
    for metadata in ["{}", r#"{ layout "custom"; }"#] {
        let mut config = new_tab_config(metadata, dir.path());
        let binding = config
            .keybinds
            .0
            .get_mut(&InputMode::Normal)
            .unwrap()
            .get_mut(&KeyWithModifier::new(BareKey::Char('t')).with_ctrl_modifier())
            .unwrap();
        assert!(format!("{:?}", binding[0]).contains("default_fg: Some("));
        // The plugin action wire carries no pane colours.
        let wire: crate::plugin_api::action::ProtobufAction =
            binding[0].clone().try_into().unwrap();
        binding[0] = wire.try_into().unwrap();
        assert!(!format!("{:?}", binding[0]).contains("default_fg: Some("));
        config
            .write_config_to_disk(config.to_string(false), &dir.path().join("config.kdl"))
            .unwrap_or_else(|e| panic!("{metadata}: {e:?}"));
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
    let deserialized = Keybinds::from_kdl(
        document.get("keybinds").unwrap(),
        Default::default(),
        &Default::default(),
    )
    .unwrap();
    let clear_defaults = true;
    let serialized = Keybinds::to_kdl(&deserialized, clear_defaults);
    let deserialized_from_serialized = Keybinds::from_kdl(
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
    let deserialized = Keybinds::from_kdl(
        document.get("keybinds").unwrap(),
        Default::default(),
        &Default::default(),
    )
    .unwrap();
    let clear_defaults = false;
    let serialized = Keybinds::to_kdl(&deserialized, clear_defaults);
    let deserialized_from_serialized = Keybinds::from_kdl(
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
    let deserialized = Keybinds::from_kdl(
        document.get("keybinds").unwrap(),
        Default::default(),
        &Default::default(),
    )
    .unwrap();
    let clear_defaults = true;
    let serialized = Keybinds::to_kdl(&deserialized, clear_defaults);
    let deserialized_from_serialized = Keybinds::from_kdl(
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
    let deserialized = Keybinds::from_kdl(
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
    let serialized = Keybinds::to_kdl(&deserialized, true);
    let deserialized_from_serialized = Keybinds::from_kdl(
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
                        hold_on_start true;
                        hold_on_close false;
                        cwd "/tmp";
                        name "my cool pane";
                    };
                }
                bind "Alt r" {
                    Run "ls" "-l" {
                        hold_on_start true;
                        hold_on_close false;
                        cwd "/tmp";
                        name "my cool pane";
                        floating true;
                    };
                }
                bind "Alt s" {
                    Run "ls" "-l" {
                        hold_on_start true;
                        hold_on_close false;
                        cwd "/tmp";
                        name "my cool pane";
                        in_place true;
                    };
                }
                bind "Alt t" { Detach; }
                bind "Alt u" {
                    LaunchOrFocusPlugin "zellij:session-manager"{
                        floating true;
                        move_to_focused_tab true;
                        skip_plugin_cache true;
                        config_key_1 "config_value_1";
                        config_key_2 "config_value_2";
                    };
                }
                bind "Alt v" {
                    LaunchOrFocusPlugin "zellij:session-manager"{
                        in_place true;
                        move_to_focused_tab true;
                        skip_plugin_cache true;
                        config_key_1 "config_value_1";
                        config_key_2 "config_value_2";
                    };
                }
                bind "Alt w" {
                    LaunchPlugin "zellij:session-manager" {
                        floating true;
                        skip_plugin_cache true;
                        config_key_1 "config_value_1";
                        config_key_2 "config_value_2";
                    };
                }
                bind "Alt x" {
                    LaunchPlugin "zellij:session-manager"{
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
                    MessagePlugin "zellij:session-manager"{
                        name "message_name";
                        payload "message_payload";
                        cwd "/tmp";
                        launch_new true;
                        skip_cache true;
                        floating true;
                        title "plugin_title";
                        config_key_1 "config_value_1";
                        config_key_2 "config_value_2";
                    };
                }
                bind "Ctrl Alt k" { FocusLastPane; }
            }
        }"#;
    let document: KdlDocument = fake_config.parse().unwrap();
    let deserialized = Keybinds::from_kdl(
        document.get("keybinds").unwrap(),
        Default::default(),
        &Default::default(),
    )
    .unwrap();
    let clear_defaults = true;
    let serialized = Keybinds::to_kdl(&deserialized, clear_defaults);
    let deserialized_from_serialized = Keybinds::from_kdl(
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
    let deserialized = Keybinds::from_kdl(
        document.get("keybinds").unwrap(),
        Default::default(),
        &Default::default(),
    )
    .unwrap();
    let clear_defaults = true;
    let serialized = Keybinds::to_kdl(&deserialized, clear_defaults);
    let deserialized_from_serialized = Keybinds::from_kdl(
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
fn keybinds_to_string_with_multiple_multiline_actions() {
    let fake_config = r#"
        keybinds {
            shared {
                bind "Ctrl n" {
                    NewPane
                    SwitchToMode "Locked"
                    MessagePlugin "zellij:session-manager"{
                        name "message_name";
                        payload "message_payload";
                        cwd "/tmp";
                        launch_new true;
                        skip_cache true;
                        floating true;
                        title "plugin_title";
                        config_key_1 "config_value_1";
                        config_key_2 "config_value_2";
                    };
                }
            }
        }"#;
    let document: KdlDocument = fake_config.parse().unwrap();
    let deserialized = Keybinds::from_kdl(
        document.get("keybinds").unwrap(),
        Default::default(),
        &Default::default(),
    )
    .unwrap();
    let clear_defaults = true;
    let serialized = Keybinds::to_kdl(&deserialized, clear_defaults);
    let deserialized_from_serialized = Keybinds::from_kdl(
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

/// The `firecracker_*` children of a `Run` action survive a serialize/parse cycle.
#[test]
fn run_action_firecracker_children_round_trip() {
    use crate::input::actions::Action;
    let fake_config = r#"
        keybinds clear-defaults=true {
            normal {
                bind "Ctrl f" {
                    Run "cargo" "test" {
                        cwd "/workspace"
                        firecracker_vm "review"
                        firecracker_image "rust-dev"
                        firecracker_worktree "/home/me/src/project"
                    }
                }
            }
        }"#;
    let document: KdlDocument = fake_config.parse().unwrap();
    let deserialized = Keybinds::from_kdl(
        document.get("keybinds").unwrap(),
        Default::default(),
        &Default::default(),
    )
    .unwrap();
    let action = deserialized
        .0
        .get(&InputMode::Normal)
        .unwrap()
        .values()
        .flatten()
        .find_map(|action| match action {
            Action::NewTiledPane { command, .. } => command.clone(),
            _ => None,
        })
        .expect("the Run action was not parsed");
    assert_eq!(action.target.firecracker_vm(), Some("review"));
    assert_eq!(action.target.firecracker_image(), Some("rust-dev"));
    assert_eq!(
        action.target.firecracker_worktree(),
        Some(std::path::Path::new("/home/me/src/project"))
    );

    let serialized = Keybinds::to_kdl(&deserialized, true);
    let reparsed = Keybinds::from_kdl(
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
    assert_eq!(deserialized, reparsed);

    // `~` is expanded, as a layout's `firecracker_worktree` is.
    let tilde: KdlDocument = r#"
        keybinds clear-defaults=true {
            normal {
                bind "Ctrl f" {
                    Run "cargo" {
                        firecracker_vm "review"
                        firecracker_worktree "~/src/project"
                    }
                }
            }
        }"#
    .parse()
    .unwrap();
    let deserialized = Keybinds::from_kdl(
        tilde.get("keybinds").unwrap(),
        Default::default(),
        &Default::default(),
    )
    .unwrap();
    let action = deserialized
        .0
        .get(&InputMode::Normal)
        .unwrap()
        .values()
        .flatten()
        .find_map(|action| match action {
            Action::NewTiledPane { command, .. } => command.clone(),
            _ => None,
        })
        .expect("the Run action was not parsed");
    let home = directories::BaseDirs::new()
        .map(|dirs| dirs.home_dir().to_path_buf())
        .expect("a home directory");
    assert_eq!(
        action.target.firecracker_worktree(),
        Some(home.join("src/project").as_path())
    );

    for (config, why) in [
        (
            r#"keybinds { normal { bind "Ctrl f" { Run "cargo" { firecracker_image "rust-dev"; }; }; }; }"#,
            "an image without a vm",
        ),
        (
            r#"keybinds { normal { bind "Ctrl f" { Run "cargo" { firecracker_vm "no/slashes"; }; }; }; }"#,
            "an invalid vm name",
        ),
        (
            r#"keybinds { normal { bind "Ctrl f" { Run "cargo" { firecracker_vm "review"; firecracker_worktree "relative/repo"; }; }; }; }"#,
            "a relative worktree",
        ),
    ] {
        let document: KdlDocument = config.parse().unwrap();
        assert!(
            Keybinds::from_kdl(
                document.get("keybinds").unwrap(),
                Default::default(),
                &Default::default(),
            )
            .is_err(),
            "{why} should be a config error"
        );
    }
}
