use super::*;

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
